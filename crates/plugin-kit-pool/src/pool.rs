//! A plug-in's voices on worker threads of its own (the CA-72's decisions.md R11, the MC-79's
//! R6): the host's audio thread and the pool's workers share each run of samples' voices, every
//! voice played whole by one of them, and the host's thread sums them in the voices' order, so
//! the output is the same to the bit as with every voice on the host's thread.
//!
//! Safe Rust only: each voice sits behind a mutex that the audio thread only ever tries (a
//! voice is free whenever no one has taken it for the run at hand); a run is offered through
//! atomics. One word holds the run's number, its voices and the next one to take, so whoever
//! takes a voice takes it by a compare-and-swap on the word, and a worker late from an earlier
//! run can take nothing from a later one. The audio thread takes voices too, until none is
//! left: any voice no worker has started, it plays itself. It then waits for the workers'
//! voices until their deadline; a voice not done by then is silent for that run and sits out
//! until its worker lets it go. In real time it waits spinning, its clock read every few looks,
//! never yielding or sleeping (the CA-72's R-DROPOUTS: a sleep gave its processor away for
//! longer than the block had left); offline, with every voice waited for, it backs off
//! ([`plugin_kit_rt::Backoff`]). Past the voices' deadline a run is still shared until the
//! block's grace ([`Pool::block`]); past that it is played on the audio thread alone, late
//! rather than silent. Nothing here allocates once the workers are started.
//!
//! A run is over when its player returns, and the word says so: a worker held up between
//! taking a voice and locking it, which finds its run over (the plug-in may have listed the
//! voice again meanwhile), leaves the voice as it is. The mark of the run a voice last played
//! for is written only by the worker holding the voice, so a late worker cannot overwrite a
//! later run's, and the audio thread counts a voice played only once it is both marked and let
//! go (the MC-79's R6).
//!
//! The workers are started off the audio thread ([`crate::Crew`]) and made audio threads at
//! once (`plugin_kit_rt::promote`: on macOS the time-constraint policy for the host's block
//! period). On macOS they also join a work interval of their own, which the first of them opens
//! at a block's first run, declaring half the period for it ([`INTERVAL`]), and closes when it
//! parks. They spin [`SPIN`] after a run (the next run of the block follows at once) and park
//! after it, or at once once the block is over ([`Pool::rest`]); the audio thread wakes as many
//! as there are voices less its own. A run carries the audio thread's floating-point flush
//! bits, which a worker takes before it plays.

use crate::budget::Budget;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

/// The most voices a run holds.
pub const MAX: usize = 16;

/// A voice's mark when it could not be played (its lock poisoned by a panic): done, but
/// silent.
const FAILED: u64 = 1 << 63;

/// The claim word's mark of a run over: its player has returned.
const CLOSED: u64 = 1 << 16;

/// How long an idle worker spins for the next run before it parks: 50 µs, not 200 (the CA-72's
/// R-SPIN; the MC-79's R30 found its workers' 200 µs spins taking a host's engine threads'
/// time). A block's runs follow each other within microseconds, so 50 µs still catches them.
pub const SPIN: Duration = Duration::from_micros(50);

/// The share of the block's period a block's work interval declares its work is to be done in
/// (macOS): half, well within the voices' deadline. Declared at the whole period, the system's
/// performance controller slowed the workers to finish just by its end (the CA-72's R27, the
/// MC-79's R13).
const INTERVAL: f64 = 0.5;

/// A voice as the plug-in and the workers share it.
pub type Shared<V> = Arc<Mutex<V>>;

/// What a voice does for a run: the plug-in's voice.
pub trait Play: Send + 'static {
    /// What a run tells every voice (its length, the rate, the side chain's samples and so on).
    type Run: Run;

    /// Plays the run into the voice's own buffer.
    fn play(&mut self, run: &Self::Run);
}

/// A run's settings as they travel from the audio thread to the workers: as words (written to
/// atomics, so that a worker late for a run reads whole words, never a torn value it then
/// plays: it can take no voice of a run that is over).
pub trait Run: Default + Send + 'static {
    /// The most words a run takes.
    const WORDS: usize;

    /// Writes the run into `words` ([`Run::WORDS`] long); returns how many it used.
    fn put(&self, words: &mut [u64]) -> usize;

    /// Reads the run back from the words [`Run::put`] used.
    fn get(&mut self, words: &[u64]);
}

/// How a pool's workers run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threads {
    /// The plug-in's: out of the budget, made audio threads for the host's blocks of this
    /// period.
    Audio(Duration),
    /// Out of the budget, as the plug-in's, but left ordinary threads: for tests of the budget
    /// that play as fast as they go.
    Budgeted,
    /// Ordinary threads, not counted against the budget nor bounded by a share: for
    /// measurements and tests that play as fast as they go.
    Ordinary,
}

impl Threads {
    /// The block period the workers are made audio threads for.
    pub fn period(self) -> Option<Duration> {
        match self {
            Threads::Audio(p) => Some(p),
            _ => None,
        }
    }

    /// Whether they come out of the budget.
    pub fn budgeted(self) -> bool {
        self != Threads::Ordinary
    }
}

/// When a run's voices must be done by (None: waited for, as long as it takes): the list's
/// first `struck` by `strikes`, the others by `voices`. A voice a worker has not finished by
/// its deadline is silent for the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Deadlines {
    pub voices: Option<Instant>,
    pub strikes: Option<Instant>,
    pub struck: usize,
}

impl Deadlines {
    /// Every voice waited for.
    pub const NONE: Deadlines = Deadlines {
        voices: None,
        strikes: None,
        struck: 0,
    };

    /// Every voice by `at`.
    pub fn at(at: Option<Instant>) -> Deadlines {
        Deadlines {
            voices: at,
            strikes: at,
            struck: 0,
        }
    }
}

thread_local! {
    /// Threads started and joined by the pools on this thread.
    static THREADS: Cell<u64> = const { Cell::new(0) };
}

/// The workers the pools have started or joined on the calling thread so far (the tests':
/// none on an audio thread).
#[doc(hidden)]
pub fn threads_here() -> u64 {
    THREADS.with(Cell::get)
}

fn counted() {
    THREADS.with(|c| c.set(c.get() + 1));
}

/// What a run offers the workers.
struct Board<V: Play> {
    /// The run's number (bits 32 up), whether it is over ([`CLOSED`]), its voices (bits 8
    /// to 15) and the next to take (bits 0 to 7).
    claim: AtomicU64,
    /// The voices' places among the plug-in's, in the order their outputs are summed.
    list: [AtomicUsize; MAX],
    /// Each voice's run number once a worker has played it for that run (written while the
    /// worker holds the voice).
    done: Box<[AtomicU64]>,
    /// The run's settings ([`Run`]), how many words of them, and the audio thread's flush bits
    /// ([`plugin_kit_rt::flush_mode`]).
    words: Box<[AtomicU64]>,
    used: AtomicUsize,
    flush: AtomicU32,
    /// The host's block is over: the workers park at once ([`Pool::rest`]).
    rest: AtomicBool,
    quit: AtomicBool,
    voices: Vec<Shared<V>>,
    /// The host's blocks so far, and the work interval the workers join, the first of them
    /// opening it at a block's first run and closing it when it parks (macOS).
    block: AtomicU64,
    interval: Option<plugin_kit_rt::WorkInterval>,
    /// For the tests: how long (µs) a worker keeps a voice after marking it played.
    #[cfg(test)]
    hold_after_mark: AtomicU64,
}

fn word(run: u32, count: usize, next: usize) -> u64 {
    (u64::from(run) << 32) | ((count as u64) << 8) | next as u64
}

/// Whether run `run` is the one on the board, and not over.
fn current<V: Play>(b: &Board<V>, run: u32) -> bool {
    let w = b.claim.load(Ordering::Acquire);
    (w >> 32) as u32 == run && w & CLOSED == 0
}

/// The next voice of run `run`, taken (its list place and its place among the voices), or
/// None once none is left (or the run is over, or another has begun).
fn take<V: Play>(b: &Board<V>, run: u32) -> Option<(usize, usize)> {
    let mut w = b.claim.load(Ordering::Acquire);
    loop {
        let (r, count, next) = (
            (w >> 32) as u32,
            ((w >> 8) & 0xff) as usize,
            (w & 0xff) as usize,
        );
        if r != run || w & CLOSED != 0 || next >= count {
            return None;
        }
        // (The place is read before the swap: while the swap succeeds, the run has voices
        // left, so its list is not yet being written for the next.)
        let slot = b.list[next].load(Ordering::Relaxed);
        match b
            .claim
            .compare_exchange_weak(w, w + 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return Some((next, slot)),
            Err(x) => w = x,
        }
    }
}

/// What a pool is started with.
pub(crate) struct Spec<'a, V> {
    pub(crate) workers: usize,
    pub(crate) voices: &'a [Shared<V>],
    pub(crate) threads: Threads,
    /// The threads' names' start (`<name>-voices-<k>`).
    pub(crate) name: &'a str,
    /// The workers the plug-in asked for, its seat's share and the share it was started at.
    pub(crate) want: usize,
    pub(crate) share: Option<Arc<AtomicUsize>>,
    pub(crate) target: usize,
}

/// The workers and their board.
pub struct Pool<V: Play> {
    board: Arc<Board<V>>,
    threads: Vec<(Thread, Option<JoinHandle<()>>)>,
    run: u32,
    /// Until when, in the present block, a run offered past its deadline is still shared
    /// ([`Pool::block`]; None: none, the run played here).
    grace: Option<Instant>,
    /// The budget its workers came out of, and how many.
    budget: Option<(&'static Budget, usize)>,
    /// What it was started for: the workers asked for, its seat's share and the share it was
    /// started at, and whether it got fewer than it was to have.
    want: usize,
    share: Option<Arc<AtomicUsize>>,
    target: usize,
    short: bool,
    /// The audio thread's words for a run's settings.
    scratch: Vec<u64>,
}

impl<V: Play> std::fmt::Debug for Pool<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("workers", &self.threads.len())
            .field("want", &self.want)
            .finish()
    }
}

impl<V: Play> Pool<V> {
    /// Up to `spec.workers` threads for these voices: out of `budget` unless ordinary, so it
    /// may start fewer, or none ([`Pool::workers`]). Not on the audio thread.
    pub(crate) fn start(budget: &'static Budget, spec: Spec<'_, V>) -> std::io::Result<Pool<V>> {
        let Spec {
            workers: asked,
            voices,
            threads,
            name,
            want,
            share,
            target,
        } = spec;
        let asked = asked.min(MAX - 1);
        let period = threads.period();
        let workers = if threads.budgeted() {
            budget.claim(asked)
        } else {
            asked
        };
        let board = Arc::new(Board {
            claim: AtomicU64::new(0),
            list: std::array::from_fn(|_| AtomicUsize::new(0)),
            done: voices.iter().map(|_| AtomicU64::new(0)).collect(),
            words: (0..V::Run::WORDS).map(|_| AtomicU64::new(0)).collect(),
            used: AtomicUsize::new(0),
            flush: AtomicU32::new(0),
            rest: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            voices: voices.to_vec(),
            block: AtomicU64::new(0),
            interval: period
                .filter(|_| workers > 0)
                .and_then(|_| plugin_kit_rt::WorkInterval::new(name)),
            #[cfg(test)]
            hold_after_mark: AtomicU64::new(0),
        });
        if period.is_some() && workers > 0 {
            plugin_kit_rt::watch();
        }
        // (The pool first: should a worker not start, those started stop and are joined and
        // the budget is given back as it drops.)
        let mut pool = Pool {
            board,
            threads: Vec::with_capacity(workers),
            run: 0,
            grace: None,
            budget: threads.budgeted().then_some((budget, workers)),
            want,
            share,
            target,
            short: workers < asked,
            scratch: vec![0; V::Run::WORDS],
        };
        for k in 0..workers {
            #[cfg(test)]
            if tests::REFUSE.get() == Some(k) {
                return Err(std::io::Error::other("refused for the test"));
            }
            let b = pool.board.clone();
            let handle = std::thread::Builder::new()
                .name(format!("{name}-voices-{k}"))
                .spawn(move || work(&b, period, k == 0))?;
            counted();
            pool.threads.push((handle.thread().clone(), Some(handle)));
        }
        Ok(pool)
    }

    /// The workers started.
    pub fn workers(&self) -> usize {
        self.threads.len()
    }

    /// The workers the plug-in asked for when it was started ([`crate::Crew::follow`]).
    pub fn want(&self) -> usize {
        self.want
    }

    /// Whether it is to be started again: its seat's share has changed, or it got fewer than
    /// its share and some is free again (the CA-72's R-THREADS). An atomic load or two: on the
    /// audio thread.
    pub fn stale(&self) -> bool {
        let moved = self
            .share
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Acquire) != self.target);
        moved || (self.short && self.budget.is_some_and(|(b, _)| b.free() > 0))
    }

    /// A host's block begins; a run offered past its voices' deadline is still shared until
    /// `grace` (the block's period's end; None: played on the audio thread alone).
    pub fn block(&mut self, grace: Option<Instant>) {
        self.grace = grace;
        self.board.block.fetch_add(1, Ordering::Release);
    }

    /// The host's block is over, its runs played: the workers park as they finish, rather than
    /// spin for a run that comes only with the next block (the CA-72's R-LO-WORKERS).
    pub fn rest(&self) {
        self.board.rest.store(true, Ordering::Release);
    }

    /// Plays the voices at `list` (places among the voices, in the order they are summed,
    /// those struck in the run first) for `run`: offered to the workers and taken by this
    /// thread too, or, past the deadline and the grace, played here. Returns which list places
    /// played by their deadlines.
    pub fn play(&mut self, list: &[usize], run: &V::Run, deadlines: Deadlines) -> [bool; MAX] {
        let count = list.len().min(MAX);
        let mut played = [false; MAX];
        let now = Instant::now();
        let past = deadlines.voices.is_some_and(|d| now >= d);
        // Past the voices' deadline the run is still shared, and waited for until the block's
        // grace: played here alone, a block of many runs took several periods once its deadline
        // had passed (the CA-72's R-DROPOUTS). Past the grace, or with none, the run is not
        // offered: the workers would take voices only for this thread to give them up at once,
        // silent for the run. Late, rather than silent.
        if self.threads.is_empty() || count < 2 || (past && self.grace.is_none_or(|g| now >= g)) {
            for (ok, &slot) in played.iter_mut().zip(&list[..count]) {
                if let Ok(mut p) = self.board.voices[slot].try_lock() {
                    p.play(run);
                    *ok = true;
                }
            }
            return played;
        }
        let number = self.offer(&list[..count], run);
        // (No more workers woken than the voices less this thread's one: the others would only
        // spin.)
        for (t, _) in self.threads.iter().take(count - 1) {
            t.unpark();
        }
        let b = &*self.board;
        // This thread's share: whatever no worker has taken.
        let mut here = [false; MAX];
        while let Some((i, slot)) = take(b, number) {
            here[i] = true;
            if let Ok(mut p) = b.voices[slot].try_lock() {
                p.play(run);
                played[i] = true;
            }
        }
        for (i, &slot) in list.iter().enumerate().take(count) {
            if here[i] {
                continue;
            }
            let deadline = if past {
                self.grace
            } else if i < deadlines.struck {
                deadlines.strikes
            } else {
                deadlines.voices
            };
            // In real time (a deadline) this thread spins, its clock read every few looks; it
            // never yields or sleeps (the CA-72's R-DROPOUTS). Offline it backs off.
            let mut wait = plugin_kit_rt::Backoff::new();
            let mut looks = 0u32;
            loop {
                let d = b.done[slot].load(Ordering::Acquire);
                if d & !FAILED == u64::from(number) {
                    if d & FAILED != 0 {
                        break;
                    }
                    // Marked: its worker lets the voice go a moment after (it marks it while
                    // holding it, so that a late worker cannot overwrite a later run's mark),
                    // and it is heard once free, the plug-in trying it to sum its samples.
                    if !matches!(
                        b.voices[slot].try_lock(),
                        Err(std::sync::TryLockError::WouldBlock)
                    ) {
                        played[i] = true;
                        break;
                    }
                }
                match deadline {
                    None => wait.snooze(),
                    Some(t) => {
                        looks = looks.wrapping_add(1);
                        if looks.is_multiple_of(16) && Instant::now() >= t {
                            break;
                        }
                        std::hint::spin_loop();
                    }
                }
            }
        }
        self.close();
        played
    }

    /// Puts a run of the voices at `list` on the board (the workers not yet woken); returns
    /// its number.
    fn offer(&mut self, list: &[usize], run: &V::Run) -> u32 {
        let b = &*self.board;
        // (Run 0 is no run: the workers start having seen it.)
        self.run = self.run.wrapping_add(1).max(1);
        for (place, &slot) in b.list.iter().zip(list) {
            place.store(slot, Ordering::Relaxed);
        }
        let used = run.put(&mut self.scratch).min(V::Run::WORDS);
        for (a, &w) in b.words.iter().zip(&self.scratch[..used]) {
            a.store(w, Ordering::Relaxed);
        }
        b.used.store(used, Ordering::Relaxed);
        b.flush
            .store(plugin_kit_rt::flush_mode(), Ordering::Relaxed);
        b.rest.store(false, Ordering::Relaxed);
        b.claim
            .store(word(self.run, list.len(), 0), Ordering::Release);
        self.run
    }

    /// The run is over, before the plug-in next tries any of its voices' locks: a worker that
    /// locks one after the plug-in has (and so after this) finds it so.
    fn close(&self) {
        self.board.claim.fetch_or(CLOSED, Ordering::AcqRel);
    }

    /// For the tests: a worker keeps a voice this long after marking it.
    #[cfg(test)]
    pub(crate) fn hold_after_mark(&self, d: Duration) {
        self.board
            .hold_after_mark
            .store(d.as_micros() as u64, Ordering::Release);
    }
}

impl<V: Play> Drop for Pool<V> {
    /// The workers stopped and joined, what they held of the budget given back. Not on the
    /// audio thread.
    fn drop(&mut self) {
        self.board.quit.store(true, Ordering::Release);
        for (t, h) in &mut self.threads {
            t.unpark();
            if let Some(h) = h.take() {
                let _ = h.join();
                counted();
            }
        }
        if let Some((budget, k)) = self.budget {
            budget.give_back(k);
        }
    }
}

/// A worker: made an audio thread, it waits for a run (spinning a moment, then parked) and
/// plays the voices it takes.
fn work<V: Play>(b: &Board<V>, period: Option<Duration>, driver: bool) {
    // What a thread sets up the first time it parks or reads the clock is done before it
    // becomes an audio thread.
    std::thread::park_timeout(Duration::from_nanos(1));
    let _ = Instant::now().elapsed();
    if let Some(period) = period {
        let _ = plugin_kit_rt::promote(period, period / 2);
    }
    let token = b
        .interval
        .as_ref()
        .and_then(plugin_kit_rt::WorkInterval::join);
    // (The first worker opens the interval at a block's first run and closes it when it parks;
    // each joins it once made an audio thread, as the CA-72's Mac measurements were taken.)
    let interval = if driver && token.is_some() {
        b.interval.as_ref().zip(period)
    } else {
        None
    };
    // A thread must leave its work interval before it ends (macOS stops the process
    // otherwise), the interval it opened closed first.
    struct Member<'a>(Option<(&'a plugin_kit_rt::WorkInterval, plugin_kit_rt::JoinToken)>);
    impl Drop for Member<'_> {
        fn drop(&mut self) {
            if let Some((i, t)) = self.0.take() {
                i.leave(t);
            }
        }
    }
    let _member = Member(b.interval.as_ref().zip(token));
    let (mut open, mut block) = (false, 0u64);
    let mut seen = 0u32;
    let mut words = vec![0u64; V::Run::WORDS];
    let mut settings = V::Run::default();
    let mut flush = plugin_kit_rt::flush_mode();
    loop {
        let since = Instant::now();
        let mut spins = 0u32;
        let number = loop {
            if b.quit.load(Ordering::Acquire) {
                if let Some((i, _)) = interval
                    && open
                {
                    i.finish();
                }
                return;
            }
            let number = (b.claim.load(Ordering::Acquire) >> 32) as u32;
            if number != seen {
                break number;
            }
            spins = spins.wrapping_add(1);
            // (Once the block is over, no spin: its next run is the next block's.)
            let rest = b.rest.load(Ordering::Acquire);
            if !rest && (!spins.is_multiple_of(64) || since.elapsed() < SPIN) {
                std::hint::spin_loop();
            } else {
                if let Some((i, _)) = interval
                    && open
                {
                    i.finish();
                    open = false;
                }
                std::thread::park();
            }
        };
        seen = number;
        if let Some((i, period)) = interval {
            let now = b.block.load(Ordering::Acquire);
            if now != block || !open {
                block = now;
                if open {
                    i.finish();
                }
                open = i.start(period.mul_f64(INTERVAL));
            }
        }
        // The run's settings, read before any voice is taken (while one can be taken, they
        // are this run's), and the audio thread's flush bits taken: a voice's samples are the
        // same whichever thread plays it.
        let used = b.used.load(Ordering::Relaxed).min(V::Run::WORDS);
        for (w, a) in words.iter_mut().zip(&b.words[..used]) {
            *w = a.load(Ordering::Relaxed);
        }
        settings.get(&words[..used]);
        let want = b.flush.load(Ordering::Relaxed);
        if want != flush {
            plugin_kit_rt::set_flush_mode(want);
            flush = want;
        }
        while let Some((_, slot)) = take(b, number) {
            let (mut p, poisoned) = match b.voices[slot].lock() {
                Ok(p) => (p, false),
                Err(e) => (e.into_inner(), true),
            };
            // Held up between taking the voice and locking it, past the run's end: the voice
            // is left as it is, neither played nor marked.
            if !current(b, number) {
                break;
            }
            let mark = if poisoned {
                u64::from(number) | FAILED
            } else {
                p.play(&settings);
                u64::from(number)
            };
            b.done[slot].store(mark, Ordering::Release);
            #[cfg(test)]
            {
                let hold = b.hold_after_mark.load(Ordering::Acquire);
                if hold > 0 {
                    std::thread::sleep(Duration::from_micros(hold));
                }
            }
            drop(p);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::thread::ThreadId;

    thread_local! {
        /// The worker [`Pool::start`] fails to start, as if the system had refused it.
        pub(crate) static REFUSE: Cell<Option<usize>> = const { Cell::new(None) };
    }

    /// A run's length alone.
    #[derive(Debug, Default, Clone, Copy, PartialEq)]
    pub(crate) struct Len(pub(crate) usize);

    impl Run for Len {
        const WORDS: usize = 1;
        fn put(&self, words: &mut [u64]) -> usize {
            words[0] = self.0 as u64;
            1
        }
        fn get(&mut self, words: &[u64]) {
            self.0 = words.first().map_or(0, |&w| w as usize);
        }
    }

    /// A voice that counts its runs and keeps where and how it played the last.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub(crate) runs: usize,
        pub(crate) n: usize,
        pub(crate) thread: Option<ThreadId>,
        pub(crate) flush: u32,
        /// Set as its next run starts, the run then held until `hold` is set.
        pub(crate) started: Option<Arc<AtomicBool>>,
        pub(crate) hold: Option<Arc<AtomicBool>>,
        /// A pause wherever it is played (so that every worker takes some).
        pub(crate) pause: Duration,
        /// A pause when played on any thread but this one (a worker's slow voice).
        pub(crate) away: Option<(ThreadId, Duration)>,
    }

    impl Play for Fake {
        type Run = Len;
        fn play(&mut self, run: &Len) {
            self.runs += 1;
            self.n = run.0;
            self.thread = Some(std::thread::current().id());
            self.flush = plugin_kit_rt::flush_mode();
            if let Some(s) = self.started.take() {
                s.store(true, Ordering::Release);
                if let Some(h) = self.hold.take() {
                    while !h.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_micros(200));
                    }
                }
            }
            if !self.pause.is_zero() {
                std::thread::sleep(self.pause);
            }
            if let Some((t, pause)) = self.away
                && self.thread != Some(t)
            {
                std::thread::sleep(pause);
            }
        }
    }

    pub(crate) fn fakes(k: usize) -> Vec<Shared<Fake>> {
        (0..k)
            .map(|_| Arc::new(Mutex::new(Fake::default())))
            .collect()
    }

    static UNBOUNDED: Budget = Budget::new(|_| usize::MAX, || usize::MAX, || None);

    fn ordinary(workers: usize, voices: &[Shared<Fake>]) -> Pool<Fake> {
        start(&UNBOUNDED, workers, voices, Threads::Ordinary)
    }

    pub(crate) fn start(
        budget: &'static Budget,
        workers: usize,
        voices: &[Shared<Fake>],
        threads: Threads,
    ) -> Pool<Fake> {
        Pool::start(
            budget,
            Spec {
                workers,
                voices,
                threads,
                name: "test",
                want: workers,
                share: None,
                target: 0,
            },
        )
        .unwrap()
    }

    /// Waits up to 10 s for `f` (bounded by what happens, not by time: it fails only if it
    /// never happens).
    pub(crate) fn until(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "{what}");
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    /// Every voice is played once a run, on whichever thread, and the workers share them.
    #[test]
    fn every_voice_is_played_once_a_run_and_the_workers_share_them() {
        let voices = fakes(4);
        for v in &voices {
            v.lock().unwrap().pause = Duration::from_millis(2);
        }
        let mut pool = ordinary(3, &voices);
        let me = std::thread::current().id();
        let mut away = 0;
        for k in 1..=20 {
            let played = pool.play(&[0, 1, 2, 3], &Len(k), Deadlines::NONE);
            assert_eq!(&played[..4], &[true; 4]);
            for v in &voices {
                let v = v.lock().unwrap();
                assert_eq!((v.runs, v.n), (k, k));
                away += usize::from(v.thread != Some(me));
            }
        }
        assert!(away > 0, "no worker played a voice");
    }

    /// One voice, or no workers: every voice on the caller's thread.
    #[test]
    fn one_voice_is_played_here() {
        let voices = fakes(2);
        let mut pool = ordinary(1, &voices);
        let me = std::thread::current().id();
        assert!(pool.play(&[1], &Len(8), Deadlines::NONE)[0]);
        assert_eq!(voices[1].lock().unwrap().thread, Some(me));
        let mut none = ordinary(0, &voices);
        assert_eq!(
            &none.play(&[0, 1], &Len(8), Deadlines::NONE)[..2],
            &[true; 2]
        );
        assert_eq!(voices[0].lock().unwrap().thread, Some(me));
    }

    /// A worker held up between taking a voice and locking it, which finds the run over when
    /// it gets the lock, leaves the voice as it is: not played, not marked.
    #[test]
    fn a_worker_late_for_a_run_over_leaves_its_voice_alone() {
        let voices = fakes(2);
        let mut pool = ordinary(1, &voices);
        let me = std::thread::current().id();
        // The worker's voice is slow away from here: past the deadline, the caller gives up.
        // (The caller's first voice is slow too, so that the worker takes the second.)
        voices[0].lock().unwrap().pause = Duration::from_millis(1);
        voices[1].lock().unwrap().away = Some((me, Duration::from_millis(30)));
        let runs = |k: usize| voices[k].lock().unwrap().runs;
        let mut late = false;
        for _ in 0..50 {
            let played = pool.play(
                &[0, 1],
                &Len(4),
                Deadlines::at(Some(Instant::now() + Duration::from_millis(2))),
            );
            late |= !played[1];
        }
        assert!(late, "the worker's slow voice was never late");
        // (Every run the worker did play it counted once.)
        assert!(runs(1) <= 50 && runs(0) == 50);
    }

    /// Past the deadline with no grace the caller's thread plays every voice itself.
    #[test]
    fn past_the_deadline_the_callers_thread_plays_every_voice() {
        let voices = fakes(3);
        let mut pool = ordinary(2, &voices);
        let me = std::thread::current().id();
        let past = Deadlines::at(Some(Instant::now()));
        pool.block(None);
        let played = pool.play(&[0, 1, 2], &Len(5), past);
        assert_eq!(&played[..3], &[true; 3]);
        for v in &voices {
            assert_eq!(v.lock().unwrap().thread, Some(me));
        }
    }

    /// Past the deadline but within the block's grace the run is still shared.
    #[test]
    fn within_the_grace_a_run_past_its_deadline_is_still_shared() {
        let voices = fakes(3);
        for v in &voices {
            v.lock().unwrap().pause = Duration::from_millis(3);
        }
        let mut pool = ordinary(2, &voices);
        let me = std::thread::current().id();
        pool.block(Some(Instant::now() + Duration::from_secs(5)));
        let mut away = false;
        for _ in 0..10 {
            let played = pool.play(&[0, 1, 2], &Len(5), Deadlines::at(Some(Instant::now())));
            assert_eq!(&played[..3], &[true; 3]);
            away |= voices.iter().any(|v| v.lock().unwrap().thread != Some(me));
        }
        assert!(away, "no worker shared a run within the grace");
    }

    /// A voice marked played is heard only once its worker lets it go.
    #[test]
    fn a_voice_marked_is_heard_once_its_worker_lets_it_go() {
        let voices = fakes(2);
        let mut pool = ordinary(1, &voices);
        let me = std::thread::current().id();
        voices[0].lock().unwrap().pause = Duration::from_millis(1);
        voices[1].lock().unwrap().away = Some((me, Duration::from_millis(1)));
        pool.hold_after_mark(Duration::from_millis(20));
        for _ in 0..10 {
            let played = pool.play(&[0, 1], &Len(4), Deadlines::NONE);
            assert_eq!(&played[..2], &[true; 2]);
            // Heard: free to try now.
            assert!(voices[1].try_lock().is_ok());
        }
    }

    /// The workers take the caller's flush bits for each run.
    #[test]
    fn the_workers_take_the_callers_flush_bits() {
        let voices = fakes(4);
        for v in &voices {
            v.lock().unwrap().pause = Duration::from_millis(2);
        }
        let mut pool = ordinary(3, &voices);
        let was = plugin_kit_rt::flush_mode();
        let all = {
            plugin_kit_rt::set_flush_mode(u32::MAX);
            let m = plugin_kit_rt::flush_mode();
            plugin_kit_rt::set_flush_mode(was);
            m
        };
        for mode in [all, 0, all] {
            plugin_kit_rt::set_flush_mode(mode);
            pool.play(&[0, 1, 2, 3], &Len(1), Deadlines::NONE);
            for v in &voices {
                assert_eq!(v.lock().unwrap().flush, mode);
            }
        }
        plugin_kit_rt::set_flush_mode(was);
    }

    /// Budgeted pools come out of one budget: never more workers together than it holds, and
    /// what a pool held is given back when it stops.
    #[test]
    fn budgeted_workers_come_out_of_one_budget() {
        static FIVE: Budget = Budget::new(|_| 5, || usize::MAX, || None);
        let voices = fakes(4);
        let a = start(&FIVE, 3, &voices, Threads::Budgeted);
        let b = start(&FIVE, 3, &voices, Threads::Budgeted);
        assert_eq!((a.workers(), b.workers()), (3, 2));
        assert!(!b.stale(), "short, with none free");
        drop(a);
        assert!(b.stale(), "short, with some free again");
        drop(b);
        assert_eq!(FIVE.held(), 0);
    }

    /// A worker the system will not start stops the others: none is left running, the budget
    /// given back.
    #[test]
    fn a_worker_not_started_stops_the_others() {
        static FOUR: Budget = Budget::new(|_| 4, || usize::MAX, || None);
        let voices = fakes(4);
        REFUSE.set(Some(2));
        let r = Pool::start(
            &FOUR,
            Spec {
                workers: 3,
                voices: &voices,
                threads: Threads::Budgeted,
                name: "test",
                want: 3,
                share: None,
                target: 0,
            },
        );
        REFUSE.set(None);
        assert!(r.is_err());
        assert_eq!(FOUR.held(), 0);
    }

    /// Resting, the workers park at once; the next run wakes them.
    #[test]
    fn a_pool_at_rest_plays_the_next_run() {
        let voices = fakes(3);
        let mut pool = ordinary(2, &voices);
        for k in 1..=5 {
            pool.block(None);
            assert_eq!(
                &pool.play(&[0, 1, 2], &Len(k), Deadlines::NONE)[..3],
                &[true; 3]
            );
            pool.rest();
            std::thread::sleep(Duration::from_millis(1));
        }
        until("a run lost", || voices[2].lock().unwrap().runs == 5);
    }
}
