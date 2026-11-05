//! The audio thread starts, joins, allocates and frees nothing, and touches no seat file,
//! while an instance's workers come and go (K-POOL): the workers wanted and not, their count
//! changed, a second instance sitting down and leaving (its share moving the first's), each
//! block's runs shared; the helper thread starts and stops the pools. Counted by a global
//! allocator on the audio thread, and by the pool's and the seats' own counts.

#![allow(unsafe_code, clippy::unwrap_used)]

use plugin_kit_pool::{
    AUTO, Ask, Budget, Crew, Deadlines, Helper, Play, Pool, Run, SERVE, Threads, seats,
    threads_here,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

// SAFETY: forwards to the system allocator; only counts while the thread is armed.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        // SAFETY: the caller's layout, passed on.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        // SAFETY: a pointer this allocator gave, with its layout.
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Default)]
struct Len(usize);

impl Run for Len {
    const WORDS: usize = 1;
    fn put(&self, words: &mut [u64]) -> usize {
        words[0] = self.0 as u64;
        1
    }
    fn get(&mut self, words: &[u64]) {
        self.0 = words[0] as usize;
    }
}

#[derive(Default)]
struct Voice {
    runs: usize,
    sum: u64,
}

impl Play for Voice {
    type Run = Len;
    fn play(&mut self, run: &Len) {
        self.runs += 1;
        // (A little work: some microseconds.)
        for k in 0..2000u64 {
            self.sum = self
                .sum
                .wrapping_add(std::hint::black_box(k * run.0 as u64));
        }
    }
}

/// An instance: its voices, its crew and helper, what its audio thread keeps.
struct Instance {
    voices: Vec<Arc<Mutex<Voice>>>,
    crew: Arc<Crew<Voice>>,
    helper: Helper,
    plan: u64,
    pool: Option<Pool<Voice>>,
    ask: Ask,
}

static BUDGET: Budget = Budget::new(|_| 6, || usize::MAX, || None);

impl Instance {
    fn new(voices: usize) -> Instance {
        let voices: Vec<_> = (0..voices)
            .map(|_| Arc::new(Mutex::new(Voice::default())))
            .collect();
        let crew = Arc::new(Crew::new(&BUDGET, "audio-test"));
        let c = crew.clone();
        let helper = Helper::start("audio-test-helper", &BUDGET, move |asks, _| {
            if asks & SERVE != 0 {
                c.serve();
            }
        })
        .unwrap();
        let plan = crew.plan(&voices, AUTO, Threads::Audio(Duration::from_micros(2667)));
        Instance {
            voices,
            crew,
            helper,
            plan,
            pool: None,
            ask: Ask::Not,
        }
    }

    /// A block, as a plug-in's audio thread plays it: the workers followed, runs shared.
    fn block(&mut self, want: usize, list: &[usize]) {
        if self
            .crew
            .follow(want, self.plan, &mut self.pool, &mut self.ask)
        {
            self.helper.ask(SERVE);
        }
        let period = Duration::from_micros(2667);
        let start = Instant::now();
        match &mut self.pool {
            Some(p) if want > 0 => {
                p.block(Some(start + period));
                for _ in 0..2 {
                    let d = Deadlines::at(Some(start + period.mul_f64(0.75)));
                    p.play(list, &Len(128), d);
                }
                p.rest();
            }
            _ => {
                for _ in 0..2 {
                    for &k in list {
                        if let Ok(mut v) = self.voices[k].try_lock() {
                            v.play(&Len(128));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn the_audio_thread_neither_starts_nor_joins_nor_allocates() {
    let mut a = Instance::new(4);
    let mut b = Instance::new(4);
    let list = [0usize, 1, 2, 3];
    // Warm: one round off the count (the first pool's threads, the clock's first reads).
    for _ in 0..50 {
        a.block(3, &list);
    }
    let (t0, io0) = (threads_here(), seats::io_here());
    let mut held_alone = 0;
    let mut held_shared = 0;
    ARMED.with(|c| c.set(true));
    for round in 0..6 {
        let want = [3, 0, 1, 3, 0, 2][round];
        for _ in 0..200 {
            a.block(want, &list);
            b.block(if round % 2 == 0 { 3 } else { 0 }, &list);
            std::thread::sleep(Duration::from_micros(300));
        }
        ARMED.with(|c| c.set(false));
        let (wa, wb) = (
            a.pool.as_ref().map_or(0, Pool::workers),
            b.pool.as_ref().map_or(0, Pool::workers),
        );
        ARMED.with(|c| c.set(true));
        if round % 2 == 0 && want == 3 {
            held_shared = held_shared.max(wa + wb);
        } else if want > 0 {
            held_alone = held_alone.max(wa);
        }
    }
    ARMED.with(|c| c.set(false));
    let (allocs, frees) = (ALLOCS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst));
    assert_eq!(
        threads_here() - t0,
        0,
        "threads started or joined on the audio thread"
    );
    assert_eq!(
        seats::io_here() - io0,
        0,
        "seat files touched on the audio thread"
    );
    assert_eq!(
        (allocs, frees),
        (0, 0),
        "allocated or freed on the audio thread"
    );
    assert_eq!(held_shared, 6, "two instances at three each");
    assert!(held_alone >= 1, "no workers alone");
    // Both let go and stopped: every worker back.
    let t = Instant::now();
    loop {
        a.block(0, &list);
        b.block(0, &list);
        if a.pool.is_none() && b.pool.is_none() && BUDGET.held() == 0 {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "workers never given back"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(BUDGET.seated(), 0, "a seat kept with nothing held");
    for v in a.voices.iter().chain(&b.voices) {
        assert!(v.lock().unwrap().runs > 0);
    }
}
