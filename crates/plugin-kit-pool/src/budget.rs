//! The budget of workers made audio threads, for every instance holding them in every process
//! of the user's (the CA-72's decisions.md R-THREADS): B for the instances seated, shared
//! evenly in the order they came, each instance's share published to it ([`Seat`]) and worked
//! out again whenever an instance sits down or leaves, here or in another process.

use crate::seats::SeatFiles;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// How often a helper thread looks at the other processes' seats ([`Budget::refresh`]).
pub const LOOK: Duration = Duration::from_millis(500);

/// How many workers the pools may hold together (`most`, for as many instances as hold seats,
/// in every process of the user's: [`crate::seats`]), how many this process's hold, an
/// instance's most (`cap`), and this process's instances holding workers, each one's share and
/// seat file.
pub struct Budget {
    most: fn(usize) -> usize,
    cap: fn() -> usize,
    held: AtomicUsize,
    seats: Mutex<Vec<SeatEntry>>,
    seated: AtomicUsize,
    /// Where the seat files are (None: this process's seats alone), opened once; the other
    /// processes' seats at the last look, and when that was.
    dir: fn() -> Option<PathBuf>,
    files: OnceLock<Option<SeatFiles>>,
    others: AtomicU64,
    looked: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("held", &self.held())
            .field("seated", &self.seated())
            .finish_non_exhaustive()
    }
}

/// One of this process's seats: its share, and the seat file it holds (None if there was none
/// to have).
struct SeatEntry {
    share: Arc<AtomicUsize>,
    file: Option<usize>,
}

/// A lock no audio thread takes.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Budget {
    /// The machine's ([`crate::rules`]), its seats shared with every process of the user's
    /// ([`crate::seats::user_dir`]): the plug-ins'.
    pub const fn machine() -> Budget {
        Budget::new(
            crate::rules::budget_workers,
            crate::rules::instance_cap,
            crate::seats::user_dir,
        )
    }

    /// One of `most` workers for so many instances seated, an instance's at most `cap`, its
    /// seat files in the folder `dir` gives (None: the process's seats alone): the tests' and
    /// the measurements'.
    pub const fn new(
        most: fn(usize) -> usize,
        cap: fn() -> usize,
        dir: fn() -> Option<PathBuf>,
    ) -> Budget {
        Budget {
            most,
            cap,
            held: AtomicUsize::new(0),
            seats: Mutex::new(Vec::new()),
            seated: AtomicUsize::new(0),
            dir,
            files: OnceLock::new(),
            others: AtomicU64::new(0),
            looked: Mutex::new(None),
        }
    }

    /// The workers this process's pools hold.
    pub fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }

    /// The instances holding seats in every process, at the last count.
    pub fn seated(&self) -> usize {
        self.seated.load(Ordering::Acquire)
    }

    /// The seats this process holds in the seat files.
    pub fn seats_here(&self) -> usize {
        self.files
            .get()
            .and_then(Option::as_ref)
            .map_or(0, |f| f.held_here().count_ones() as usize)
    }

    /// An instance's most workers.
    pub(crate) fn cap(&self) -> usize {
        (self.cap)()
    }

    /// The most for the instances seated now (at least one).
    fn most(&self) -> usize {
        (self.most)(self.seated().max(1))
    }

    /// How many are free now (of this process's count).
    pub(crate) fn free(&self) -> usize {
        self.most().saturating_sub(self.held())
    }

    /// The seat files, opened the first time (never on an audio thread: seats are taken and
    /// given up only where pools are started and stopped).
    fn files(&self) -> Option<&SeatFiles> {
        self.files
            .get_or_init(|| (self.dir)().and_then(|d| SeatFiles::open(&d)))
            .as_ref()
    }

    /// A seat for an instance, its share from now on, a seat file taken with it; every seat's
    /// share worked out again, the other processes' counted now.
    pub(crate) fn sit(&'static self) -> Seat {
        let share = Arc::new(AtomicUsize::new(0));
        let mut seats = lock(&self.seats);
        let file = self.files().and_then(SeatFiles::take);
        seats.push(SeatEntry {
            share: share.clone(),
            file,
        });
        self.look_and_reshare(&seats);
        Seat {
            budget: self,
            share,
        }
    }

    /// The other processes' seats counted now ([`SeatFiles::others`]), and the shares worked
    /// out again for them.
    fn look_and_reshare(&self, seats: &[SeatEntry]) {
        let others = self.files().map_or(0, SeatFiles::others);
        self.others.store(others, Ordering::Release);
        *lock(&self.looked) = Some(Instant::now());
        self.reshare(seats, others);
    }

    /// The other processes' seats looked at again, at most every [`LOOK`], while this process
    /// holds any; the shares worked out again if they have changed. A helper thread's (never an
    /// audio thread's).
    pub fn refresh(&self) {
        let seats = lock(&self.seats);
        if seats.is_empty() || self.files().is_none() {
            return;
        }
        if lock(&self.looked).is_some_and(|t| t.elapsed() < LOOK) {
            return;
        }
        let before = self.others.load(Ordering::Acquire);
        let others = self.files().map_or(0, SeatFiles::others);
        *lock(&self.looked) = Some(Instant::now());
        if others != before {
            self.others.store(others, Ordering::Release);
            self.reshare(&seats, others);
        }
    }

    /// [`Budget::refresh`] now, whenever it last looked (the tests').
    #[doc(hidden)]
    pub fn refresh_now(&self) {
        *lock(&self.looked) = None;
        self.refresh();
    }

    /// Every seat's share: the most for every instance seated in every process, evenly, the
    /// remainder to the first seats (the seat files' order, then this process's instances that
    /// hold none).
    fn reshare(&self, seats: &[SeatEntry], others: u64) {
        let mine = seats
            .iter()
            .filter_map(|e| e.file)
            .fold(0u64, |m, k| m | (1 << k));
        let filed = (others | mine).count_ones() as usize;
        let n = filed + seats.iter().filter(|e| e.file.is_none()).count();
        self.seated.store(n, Ordering::Release);
        if n == 0 {
            return;
        }
        let most = (self.most)(n);
        let mut unfiled = 0;
        for e in seats {
            let rank = match e.file {
                Some(k) => ((others | mine) & ((1u64 << k) - 1)).count_ones() as usize,
                None => {
                    unfiled += 1;
                    filed + unfiled - 1
                }
            };
            e.share
                .store(crate::rules::share(most, n, rank), Ordering::Release);
        }
    }

    /// Up to `want` workers: as many as are left.
    pub(crate) fn claim(&self, want: usize) -> usize {
        let most = self.most();
        let mut held = self.held();
        loop {
            let k = want.min(most.saturating_sub(held));
            if k == 0 {
                return 0;
            }
            match self.held.compare_exchange_weak(
                held,
                held + k,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return k,
                Err(h) => held = h,
            }
        }
    }

    pub(crate) fn give_back(&self, k: usize) {
        self.held.fetch_sub(k, Ordering::AcqRel);
    }
}

/// An instance's place in the budget while it holds workers: its share, which moves as others
/// come and go, in this process or another. Given up when dropped (its seat file let go), the
/// others' shares worked out again.
pub(crate) struct Seat {
    budget: &'static Budget,
    pub(crate) share: Arc<AtomicUsize>,
}

impl Drop for Seat {
    fn drop(&mut self) {
        let b = self.budget;
        let mut seats = lock(&b.seats);
        if let Some(at) = seats
            .iter()
            .position(|e| Arc::ptr_eq(&e.share, &self.share))
        {
            let e = seats.remove(at);
            if let (Some(k), Some(files)) = (e.file, b.files()) {
                files.give_back(k);
            }
        }
        b.look_and_reshare(&seats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);

    /// Seats share the budget evenly in the order they came, and the shares follow them as
    /// they leave.
    #[test]
    fn seats_share_the_budget_evenly() {
        let a = SIX.sit();
        assert_eq!(a.share.load(Ordering::Acquire), 6);
        let b = SIX.sit();
        let c = SIX.sit();
        let shares = |s: &[&Seat]| -> Vec<usize> {
            s.iter().map(|s| s.share.load(Ordering::Acquire)).collect()
        };
        assert_eq!(shares(&[&a, &b, &c]), [2, 2, 2]);
        let d = SIX.sit();
        assert_eq!(shares(&[&a, &b, &c, &d]), [2, 2, 1, 1]);
        assert_eq!(SIX.seated(), 4);
        drop(a);
        assert_eq!(shares(&[&b, &c, &d]), [2, 2, 2]);
        drop((b, c));
        assert_eq!(shares(&[&d]), [6]);
        drop(d);
        assert_eq!(SIX.seated(), 0);
    }

    /// Another process's instances count: a process holding two seats in the same folder
    /// makes this instance's share a third of the budget for three, and it is all this
    /// instance's again once that process is killed (the kernel lets its locks go).
    #[test]
    fn the_shares_follow_other_processes_instances() {
        static DIR: OnceLock<PathBuf> = OnceLock::new();
        static NINE: Budget = Budget::new(|_| 9, || usize::MAX, || DIR.get().cloned());
        let dir = tempfile::tempdir().unwrap();
        DIR.set(dir.path().to_owned()).unwrap();
        let mut child = crate::seats::tests::holder(dir.path(), 2);
        let seat = NINE.sit();
        assert_eq!(NINE.seated(), 3);
        assert_eq!(NINE.seats_here(), 1);
        let share = seat.share.load(Ordering::Acquire);
        assert_eq!(share, 3, "the other process's seats not counted");
        child.kill().unwrap();
        child.wait().unwrap();
        NINE.refresh_now();
        assert_eq!(NINE.seated(), 1);
        assert_eq!(seat.share.load(Ordering::Acquire), 9);
        drop(seat);
        assert_eq!(NINE.seats_here(), 0);
    }

    /// Claims never take more than the budget together, and what is given back is free again.
    #[test]
    fn claims_stay_within_the_budget() {
        static FOUR: Budget = Budget::new(|_| 4, || usize::MAX, || None);
        assert_eq!(FOUR.claim(3), 3);
        assert_eq!(FOUR.claim(3), 1);
        assert_eq!(FOUR.claim(1), 0);
        assert_eq!(FOUR.free(), 0);
        FOUR.give_back(3);
        assert_eq!(FOUR.free(), 3);
        FOUR.give_back(1);
        assert_eq!(FOUR.held(), 0);
    }
}
