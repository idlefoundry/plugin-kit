//! An instance's workers, held only while it needs them (the CA-72's decisions.md R21 and
//! R-THREADS): its audio thread asks for a pool and lets it go ([`Crew::follow`]), never
//! waiting and never starting or stopping a thread; a helper thread ([`crate::Helper`]) stops
//! the pools let go and starts the one asked for ([`Crew::serve`]), for the voices last planned
//! ([`Crew::plan`]), within the instance's share of the budget. A pool started for a plan no
//! longer asked for (the voices built anew, the plug-in deactivated, the workers no longer
//! wanted) is stopped where it was started, so no worker is kept from the budget.

use crate::budget::{Budget, Seat, lock};
use crate::pool::{Play, Pool, Shared, Spec, Threads};
use std::sync::Mutex;
use std::sync::atomic::Ordering;

/// [`Crew::plan`]'s `workers` for as many as the instance's share and the machine's cap give.
pub const AUTO: usize = usize::MAX;

/// An instance's workers: what they are for, and what passes between its audio thread and its
/// helper thread.
pub struct Crew<V: Play> {
    budget: &'static Budget,
    name: &'static str,
    /// Held while the helper thread serves, so that a new plan waits for a pool being started
    /// for an earlier one, and stops it before a pool of its own claims workers.
    serving: Mutex<()>,
    plan: Mutex<Plan<V>>,
    hand: Mutex<Hand<V>>,
    /// The instance's seat in the budget while it holds or asks for workers (the helper
    /// thread's).
    seat: Mutex<Option<Seat>>,
}

/// What a pool is started for: the voices, how many workers at most and how they run; a new
/// generation each time it is set.
struct Plan<V> {
    generation: u64,
    voices: Vec<Shared<V>>,
    workers: usize,
    threads: Threads,
}

/// What passes between the audio thread and the helper thread.
struct Hand<V: Play> {
    /// The plan's generation the audio thread asks a pool for (0: none), and how many workers.
    asked: u64,
    want: usize,
    /// The answer for a generation: its pool, or none (the system would not start one).
    ready: Option<(u64, Option<Pool<V>>)>,
    /// A pool the audio thread let go, to be stopped.
    back: Option<Pool<V>>,
}

/// Where the audio thread's ask for workers stands, for which plan's generation
/// ([`Crew::plan`]): an ask or answer for an earlier plan counts as none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ask {
    #[default]
    Not,
    /// Asked; until the answer comes, the voices play on the audio thread.
    Waiting(u64),
    /// Answered: a pool, or none, until the workers are next wanted anew.
    Answered(u64),
}

impl<V: Play> std::fmt::Debug for Crew<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Crew")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl<V: Play> Crew<V> {
    /// With nothing planned, its workers from `budget`, named `<name>-voices-<k>`. Not on the
    /// audio thread.
    pub fn new(budget: &'static Budget, name: &'static str) -> Crew<V> {
        let crew = Crew {
            budget,
            name,
            serving: Mutex::new(()),
            plan: Mutex::new(Plan {
                generation: 0,
                voices: Vec::new(),
                workers: 0,
                threads: Threads::Ordinary,
            }),
            hand: Mutex::new(Hand {
                asked: 0,
                want: 0,
                ready: None,
                back: None,
            }),
            seat: Mutex::new(None),
        };
        // (Its lock taken once here, off the audio thread: on macOS the standard library may
        // make a mutex's lock on its first use.)
        drop(crew.hand.try_lock());
        crew
    }

    /// Pools from now on for up to `workers` workers ([`AUTO`]: as many as the share gives)
    /// sharing `voices`, run as `threads` says; returns the plan's generation. Any pool started
    /// or let go for an earlier plan is stopped, here (one being started is waited for), its ask
    /// withdrawn and the seat given up. Not on the audio thread.
    pub fn plan(&self, voices: &[Shared<V>], workers: usize, threads: Threads) -> u64 {
        let _serving = lock(&self.serving);
        // (On macOS the standard library makes a mutex's lock on its first use: each voice's
        // taken once here, off the audio thread.)
        for v in voices {
            drop(v.lock());
        }
        let (generation, old) = {
            let mut p = lock(&self.plan);
            p.generation += 1;
            p.workers = workers;
            p.threads = threads;
            (
                p.generation,
                std::mem::replace(&mut p.voices, voices.to_vec()),
            )
        };
        drop(old);
        let stop = {
            let mut h = lock(&self.hand);
            h.asked = 0;
            (h.ready.take(), h.back.take())
        };
        drop(stop);
        drop(lock(&self.seat).take());
        generation
    }

    /// Nothing planned: as [`Crew::plan`], for no voices.
    pub fn clear(&self) {
        self.plan(&[], 0, Threads::Ordinary);
    }

    /// A pool of up to `want` workers for plan `generation` while it is the plan, or none
    /// (another planned since, nothing wanted, or the system would not start one): within the
    /// instance's share of the budget and the machine's cap, never more than the voices less
    /// one, its seat taken if it has none. Not on the audio thread.
    pub fn start(&self, generation: u64, want: usize) -> Option<Pool<V>> {
        let p = lock(&self.plan);
        if p.generation != generation || p.voices.len() < 2 || p.workers == 0 || want == 0 {
            return None;
        }
        let asked = want.min(p.workers).min(p.voices.len() - 1);
        // (The most it may have whatever its share; at the share now, the workers it is to have.
        // It is started again only when that number would change: `Pool::stale`.)
        let (limit, share, target) = if p.threads.budgeted() {
            let share = lock(&self.seat)
                .get_or_insert_with(|| self.budget.sit())
                .share
                .clone();
            let limit = asked.min(self.budget.cap());
            let target = limit.min(share.load(Ordering::Acquire));
            (limit, Some(share), target)
        } else {
            (asked, None, asked)
        };
        let pool = Pool::start(
            self.budget,
            Spec {
                workers: target,
                voices: &p.voices,
                threads: p.threads,
                name: self.name,
                generation,
                want,
                limit,
                share,
                target,
            },
        )
        .ok();
        // (The system would not start it: the instance holds nothing, so neither a seat.)
        if pool.is_none() {
            drop(lock(&self.seat).take());
        }
        pool
    }

    /// The audio thread's asks served: the pools it let go stopped (their workers back in the
    /// budget), then a pool started for the plan it asks for, unless one is ready. Not on the
    /// audio thread: the helper thread's.
    pub fn serve(&self) {
        let _serving = lock(&self.serving);
        if let Some((generation, want)) = self.tidy() {
            self.deliver(generation, self.start(generation, want));
        }
    }

    /// The pools let go, or ready for an ask withdrawn, stopped (and with nothing asked for,
    /// the seat given up); the generation and workers asked for, if a pool is to be started.
    fn tidy(&self) -> Option<(u64, usize)> {
        let (stop, start, idle) = {
            let mut h = lock(&self.hand);
            let asked = h.asked;
            let stale = h.ready.take_if(|(g, _)| *g != asked);
            let start = (asked != 0 && h.ready.is_none()).then_some((asked, h.want));
            let back = h.back.take();
            let idle = asked == 0 && h.ready.is_none() && (back.is_some() || stale.is_some());
            ((back, stale), start, idle)
        };
        // (Stopped once the lock is let go: the audio thread may be trying it.)
        drop(stop);
        if idle {
            drop(lock(&self.seat).take());
        }
        start
    }

    /// The answer for plan `generation` to the audio thread, while it still asks for it and
    /// has none; else its pool stopped here, and, with nothing asked for, ready or let go, the
    /// seat given up (the ask was withdrawn while the pool was being started: nothing else
    /// would give it up).
    fn deliver(&self, generation: u64, pool: Option<Pool<V>>) {
        let (late, idle) = {
            let mut h = lock(&self.hand);
            if h.asked == generation && h.ready.is_none() {
                h.ready = Some((generation, pool));
                (None, false)
            } else {
                let idle = h.asked == 0 && h.ready.is_none() && h.back.is_none();
                (pool, idle)
            }
        };
        // (Stopped once the lock is let go: the audio thread may be trying it.)
        drop(late);
        if idle {
            drop(lock(&self.seat).take());
        }
    }

    /// At a block's start, on the audio thread, without waiting: its `pool` and `ask` brought
    /// towards `want` workers (0: none) of plan `generation`. Wanted, a pool started for the
    /// plan is taken once it is ready, or asked for, once (until it comes, the caller plays
    /// every voice itself); a pool for another plan or count, or whose share has moved so that
    /// it would have another number of workers, is let go and asked for again; an ask or answer
    /// for another plan counts as none. Not wanted, the pool is let go to be stopped off this thread and the
    /// ask withdrawn. A pool moves; none is started or stopped here, and nothing is allocated
    /// or freed. Should the helper thread hold the lock, this is tried again at the next block.
    /// Returns whether [`Crew::serve`] is due (the helper thread to be asked).
    pub fn follow(
        &self,
        want: usize,
        generation: u64,
        pool: &mut Option<Pool<V>>,
        ask: &mut Ask,
    ) -> bool {
        if let Ask::Waiting(g) | Ask::Answered(g) = *ask
            && g != generation
        {
            *ask = Ask::Not;
        }
        let again = want > 0
            && pool
                .as_ref()
                .is_some_and(|p| p.generation() != generation || p.want() != want || p.stale());
        let settled = if want > 0 {
            !again && (pool.is_some() || *ask == Ask::Answered(generation))
        } else {
            pool.is_none() && *ask == Ask::Not
        };
        if settled || generation == 0 {
            return false;
        }
        let Ok(mut h) = self.hand.try_lock() else {
            return false;
        };
        if want > 0 {
            if again {
                if h.back.is_some() {
                    // (The last one let go not stopped yet: asked again at a later block.)
                    return true;
                }
                h.back = pool.take();
                h.asked = generation;
                h.want = want;
                *ask = Ask::Waiting(generation);
                return true;
            }
            // (With no pool here: taking the answer drops nothing. One may be ready for an ask
            // withdrawn and made again since.)
            if let Some((_, answer)) = h.ready.take_if(|(g, _)| *g == generation) {
                *pool = answer;
                h.asked = 0;
                *ask = Ask::Answered(generation);
                return false;
            }
            if *ask == Ask::Not {
                h.asked = generation;
                h.want = want;
                *ask = Ask::Waiting(generation);
                return true;
            }
            // (Asked for another count meanwhile: the answer, when it comes, is let go again.)
            h.want = want;
            return false;
        }
        // A pool started for an ask withdrawn is stopped by the helper thread: a pool ready,
        // at once, or one being started, as it comes.
        if h.asked == generation {
            h.asked = 0;
        }
        *ask = Ask::Not;
        let mut due = h.ready.is_some();
        if pool.is_some() && h.back.is_none() {
            h.back = pool.take();
            due = true;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::Deadlines;
    use crate::pool::tests::{Fake, Len, fakes, until};

    fn plays(pool: &mut Option<Pool<Fake>>, voices: usize) {
        let list: Vec<usize> = (0..voices).collect();
        if let Some(p) = pool {
            p.play(&list, &Len(4), Deadlines::NONE);
        }
    }

    /// Workers only while wanted: none until asked, a pool once the helper serves, given back
    /// to the budget (and the seat given up) once no longer wanted, and asked again.
    #[test]
    fn a_crew_holds_workers_only_while_wanted() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let voices = fakes(4);
        let crew = Crew::new(&SIX, "test");
        let g = crew.plan(&voices, AUTO, Threads::Budgeted);
        let (mut pool, mut ask) = (None, Ask::Not);
        assert!(!crew.follow(0, g, &mut pool, &mut ask), "nothing wanted");
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
        for round in 0..3 {
            assert!(crew.follow(3, g, &mut pool, &mut ask), "round {round}");
            assert!(pool.is_none() && ask == Ask::Waiting(g));
            crew.serve();
            assert!(!crew.follow(3, g, &mut pool, &mut ask));
            assert_eq!(pool.as_ref().map(Pool::workers), Some(3));
            assert_eq!((SIX.held(), SIX.seated()), (3, 1));
            plays(&mut pool, 4);
            assert!(crew.follow(0, g, &mut pool, &mut ask));
            assert!(pool.is_none());
            assert_eq!(SIX.held(), 3, "stopped on the audio thread");
            crew.serve();
            assert_eq!((SIX.held(), SIX.seated()), (0, 0), "not given back");
        }
    }

    /// A new count of workers wanted lets the pool go and asks for one of that count.
    #[test]
    fn a_new_count_starts_the_pool_again() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let voices = fakes(4);
        let crew = Crew::new(&SIX, "test");
        let g = crew.plan(&voices, AUTO, Threads::Budgeted);
        let (mut pool, mut ask) = (None, Ask::Not);
        crew.follow(1, g, &mut pool, &mut ask);
        crew.serve();
        crew.follow(1, g, &mut pool, &mut ask);
        assert_eq!(pool.as_ref().map(Pool::workers), Some(1));
        assert!(crew.follow(3, g, &mut pool, &mut ask));
        crew.serve();
        crew.follow(3, g, &mut pool, &mut ask);
        assert_eq!(pool.as_ref().map(Pool::workers), Some(3));
        assert_eq!(SIX.held(), 3);
        // (More than the voices less one: the voices less one.)
        assert!(crew.follow(9, g, &mut pool, &mut ask));
        crew.serve();
        crew.follow(9, g, &mut pool, &mut ask);
        assert_eq!(pool.as_ref().map(Pool::workers), Some(3));
        crew.clear();
        drop(pool.take());
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
    }

    /// Deactivated (the plan cleared) with a pool held, asked for or ready: every worker given
    /// back and the seat given up once the audio thread lets its pool go.
    #[test]
    fn a_cleared_plan_gives_every_worker_back() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let voices = fakes(4);
        let crew = Crew::new(&SIX, "test");
        let g = crew.plan(&voices, AUTO, Threads::Budgeted);
        let (mut pool, mut ask) = (None, Ask::Not);
        crew.follow(2, g, &mut pool, &mut ask);
        crew.serve();
        // (Ready, not yet taken.)
        crew.clear();
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
        // An old generation's pool taken before the clear is let go as a plugin does.
        let g = crew.plan(&voices, AUTO, Threads::Budgeted);
        let (mut pool, mut ask) = (None, Ask::Not);
        crew.follow(2, g, &mut pool, &mut ask);
        crew.serve();
        crew.follow(2, g, &mut pool, &mut ask);
        assert_eq!(SIX.held(), 2);
        drop(pool.take());
        crew.clear();
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
    }

    /// The shares follow the instances: a second instance sitting down moves the first's
    /// share, which restarts its pool at it; the first leaving gives the second all.
    #[test]
    fn the_shares_follow_the_instances() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let (va, vb) = (fakes(10), fakes(10));
        let (a, b) = (Crew::new(&SIX, "a"), Crew::new(&SIX, "b"));
        let (ga, gb) = (
            a.plan(&va, AUTO, Threads::Budgeted),
            b.plan(&vb, AUTO, Threads::Budgeted),
        );
        let (mut pa, mut aa, mut pb, mut ab) = (None, Ask::Not, None, Ask::Not);
        let settle = |pa: &mut Option<Pool<Fake>>,
                      aa: &mut Ask,
                      pb: &mut Option<Pool<Fake>>,
                      ab: &mut Ask,
                      b_on: bool| {
            for _ in 0..8 {
                if a.follow(9, ga, pa, aa) {
                    a.serve();
                }
                if b.follow(if b_on { 9 } else { 0 }, gb, pb, ab) {
                    b.serve();
                }
            }
        };
        settle(&mut pa, &mut aa, &mut pb, &mut ab, false);
        assert_eq!(pa.as_ref().map(Pool::workers), Some(6));
        settle(&mut pa, &mut aa, &mut pb, &mut ab, true);
        assert_eq!(
            (
                pa.as_ref().map(Pool::workers),
                pb.as_ref().map(Pool::workers)
            ),
            (Some(3), Some(3))
        );
        assert_eq!(SIX.held(), 6);
        settle(&mut pa, &mut aa, &mut pb, &mut ab, false);
        assert_eq!(pa.as_ref().map(Pool::workers), Some(6));
        assert!(pb.is_none());
        drop(pa.take());
        a.clear();
        b.clear();
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
    }

    /// An ask withdrawn while the helper is starting its pool (UNISON back to 1 meanwhile):
    /// the late pool is stopped and the seat given up, nothing kept from the budget.
    #[test]
    fn an_ask_withdrawn_while_its_pool_starts_gives_up_the_seat() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let voices = fakes(4);
        let crew = Crew::new(&SIX, "test");
        let g = crew.plan(&voices, AUTO, Threads::Budgeted);
        let (mut pool, mut ask) = (None, Ask::Not);
        assert!(crew.follow(1, g, &mut pool, &mut ask));
        // (The helper's serve, step by step: the ask taken, then the pool started.)
        let (generation, want) = crew.tidy().expect("nothing asked");
        assert!(!crew.follow(0, g, &mut pool, &mut ask), "withdrawn");
        let started = crew.start(generation, want);
        assert_eq!(started.as_ref().map(Pool::workers), Some(1));
        assert_eq!(SIX.seated(), 1);
        crew.deliver(generation, started);
        assert_eq!(
            (SIX.held(), SIX.seated()),
            (0, 0),
            "a seat or a worker kept"
        );
        assert!(pool.is_none() && ask == Ask::Not);
    }

    /// A pool whose share moves but still covers its workers is kept: a pool of one is not
    /// started again when its share goes from six to three or two; one of three is, at two.
    #[test]
    fn a_pool_is_kept_while_its_share_still_covers_it() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let (va, vb, vc) = (fakes(4), fakes(4), fakes(4));
        let (a, b, c) = (
            Crew::new(&SIX, "a"),
            Crew::new(&SIX, "b"),
            Crew::new(&SIX, "c"),
        );
        let ga = a.plan(&va, AUTO, Threads::Budgeted);
        let gb = b.plan(&vb, AUTO, Threads::Budgeted);
        let gc = c.plan(&vc, AUTO, Threads::Budgeted);
        let pa = a.start(ga, 1).unwrap();
        assert_eq!(pa.workers(), 1);
        let pb = b.start(gb, 3).unwrap();
        assert_eq!(pb.workers(), 3);
        assert!(!pa.stale(), "one worker started again at a share of three");
        assert!(!pb.stale());
        let pc = c.start(gc, 3).unwrap();
        assert!(!pa.stale(), "one worker started again at a share of two");
        assert!(pb.stale(), "three workers kept at a share of two");
        drop((pa, pb, pc));
        for crew in [&a, &b, &c] {
            crew.clear();
        }
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
    }

    /// A pool started for an earlier plan is let go and one for the new plan asked for, even at
    /// the same count and share (its voices are the old plan's); an ask made before a new plan
    /// is made again for it.
    #[test]
    fn a_new_plan_is_followed_whatever_the_ask() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let (old, new) = (fakes(4), fakes(6));
        let crew = Crew::new(&SIX, "test");
        let g1 = crew.plan(&old, AUTO, Threads::Budgeted);
        let (mut pool, mut ask) = (None, Ask::Not);
        crew.follow(2, g1, &mut pool, &mut ask);
        crew.serve();
        crew.follow(2, g1, &mut pool, &mut ask);
        assert_eq!(pool.as_ref().map(Pool::generation), Some(g1));
        let g2 = crew.plan(&new, AUTO, Threads::Budgeted);
        assert!(
            crew.follow(2, g2, &mut pool, &mut ask),
            "the old plan's pool kept"
        );
        assert!(pool.is_none() && ask == Ask::Waiting(g2));
        crew.serve();
        crew.follow(2, g2, &mut pool, &mut ask);
        assert_eq!(pool.as_ref().map(Pool::generation), Some(g2));
        plays(&mut pool, 6);
        drop(pool.take());
        // Asked, then a new plan before the answer.
        let g3 = crew.plan(&old, AUTO, Threads::Budgeted);
        let (mut pool, mut ask) = (None, Ask::Not);
        assert!(crew.follow(2, g3, &mut pool, &mut ask));
        let g4 = crew.plan(&new, AUTO, Threads::Budgeted);
        assert!(crew.follow(2, g4, &mut pool, &mut ask), "never asked again");
        crew.serve();
        crew.follow(2, g4, &mut pool, &mut ask);
        assert_eq!(pool.as_ref().map(Pool::generation), Some(g4));
        drop(pool.take());
        crew.clear();
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
    }

    /// A pool the system would not start gives its seat back: the instance holds nothing.
    #[test]
    fn a_pool_refused_gives_its_seat_back() {
        static SIX: Budget = Budget::new(|_| 6, || usize::MAX, || None);
        let voices = fakes(4);
        let crew = Crew::new(&SIX, "test");
        let g = crew.plan(&voices, AUTO, Threads::Budgeted);
        crate::pool::tests::REFUSE.set(Some(0));
        let pool = crew.start(g, 3);
        crate::pool::tests::REFUSE.set(None);
        assert!(pool.is_none());
        assert_eq!(
            (SIX.held(), SIX.seated()),
            (0, 0),
            "a seat or a worker kept"
        );
        assert_eq!(
            crew.start(g, 3).map(|p| p.workers()),
            Some(3),
            "not tried again"
        );
        crew.clear();
        assert_eq!((SIX.held(), SIX.seated()), (0, 0));
    }

    /// Ordinary workers (measurements) take no seat and are bounded by nothing but the voices.
    #[test]
    fn ordinary_workers_take_no_seat() {
        static ONE: Budget = Budget::new(|_| 1, || 1, || None);
        let voices = fakes(5);
        let crew = Crew::new(&ONE, "test");
        let g = crew.plan(&voices, 4, Threads::Ordinary);
        let mut pool = crew.start(g, AUTO);
        assert_eq!(pool.as_ref().map(Pool::workers), Some(4));
        assert_eq!((ONE.held(), ONE.seated()), (0, 0));
        plays(&mut pool, 5);
        until("not every voice played", || {
            voices.iter().all(|v| v.lock().unwrap().runs == 1)
        });
    }
}
