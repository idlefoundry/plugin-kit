//! How many workers: the CA-72's rules (its decisions.md R11, R39 and R-THREADS, approved by
//! the owner on 2026-10-16), the same for every plug-in on the kit.
//!
//! - **The budget**, B: every instance holding workers, in every process of the user's
//!   ([`crate::seats`]), shares B = max(floor, N − 2 − A) workers, where N is the machine's
//!   hardware threads and A the instances holding workers (each has a host thread of its own;
//!   two are left for the system and the host). The floor is [`workers_for`] N, the count
//!   before R-THREADS. On macOS also at least the instances and two more, never more than
//!   N − 2 ([`budget_for_macos`]).
//! - **An instance's cap**, C = max(floor, F − 2), F the fast cores, so that one instance's
//!   voices stay on them; and never more than its voices less one (a worker without a voice of
//!   its own does nothing), which the plug-in says in what it asks for.
//! - **Shares are even** ([`share`]), the remainder to the first seats.

use crate::machine::{Cores, cores};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The workers for a machine of `n` hardware threads before R-THREADS (the CA-72's R11, R39),
/// now the least the budget holds: a third of them less two, at most four, and at least one
/// from 3 up (so that on a quad-core the voices are not all on the host's thread while its
/// other processors idle); none on 1 or 2, where the host's thread and a worker would be all
/// the machine has.
pub fn workers_for(n: usize) -> usize {
    if n < 3 {
        0
    } else {
        (n.saturating_sub(2) / 3).clamp(1, 4)
    }
}

/// B on a machine of `all` hardware threads while `instances` hold workers (Linux, Windows).
pub fn budget_for(all: usize, instances: usize) -> usize {
    workers_for(all).max(all.saturating_sub(2 + instances))
}

/// B on a Mac of `all` hardware threads (the CA-72's R-THREADS, the macOS rule approved
/// 2026-10-16): [`budget_for`]'s, or the instances and two over if more, never more than the
/// hardware threads less two. macOS demotes a time-constraint thread that runs far past its
/// computation, so the margin for the instances' host threads is not needed there as on Linux
/// at `SCHED_FIFO`.
pub fn budget_for_macos(all: usize, instances: usize) -> usize {
    budget_for(all, instances).max((instances + 2).min(all.saturating_sub(2)))
}

/// C on a machine of these cores.
pub fn cap_for(c: Cores) -> usize {
    workers_for(c.all).max(c.fast.saturating_sub(2))
}

/// The share of `most` workers of the seat ranked `rank` (from 0) among `seats`: even, the
/// remainder to the first.
pub fn share(most: usize, seats: usize, rank: usize) -> usize {
    if seats == 0 {
        return 0;
    }
    most / seats + usize::from(rank < most % seats)
}

/// A measurement's own budget (0: the machine's rule).
static OVERRIDE: AtomicUsize = AtomicUsize::new(0);

/// B on this machine for `instances` holding workers, or a measurement's own
/// ([`set_budget_for_measurement`]).
pub fn budget_workers(instances: usize) -> usize {
    match OVERRIDE.load(Ordering::Relaxed) {
        0 if cfg!(target_os = "macos") => budget_for_macos(cores().all, instances),
        0 => budget_for(cores().all, instances),
        n => n,
    }
}

/// C on this machine; none while a measurement sets the budget (its share alone bounds it).
pub fn instance_cap() -> usize {
    if OVERRIDE.load(Ordering::Relaxed) != 0 {
        return usize::MAX;
    }
    cap_for(cores())
}

/// Sets the budget for a measurement (0: back to the machine's rule), an instance then bounded
/// by its share alone. Not for a plug-in: the benches' way to try other counts.
#[doc(hidden)]
pub fn set_budget_for_measurement(workers: usize) {
    OVERRIDE.store(workers, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What an instance holds: its share, the cap and its voices less one.
    fn holds(most: usize, seats: usize, cap: usize, voices: usize) -> Vec<usize> {
        (0..seats)
            .map(|r| share(most, seats, r).min(cap).min(voices - 1))
            .collect()
    }

    /// The floor: none on 1 or 2, one on 3 to 7, two on 8, four from 14.
    #[test]
    fn the_floor_is_the_count_before() {
        let got: Vec<usize> = [1, 2, 3, 4, 5, 7, 8, 11, 14, 16, 20, 64]
            .iter()
            .map(|&n| workers_for(n))
            .collect();
        assert_eq!(got, [0, 0, 1, 1, 1, 1, 2, 3, 4, 4, 4, 4]);
    }

    /// B, C and the shares on the three machines measured (the CA-72's R-THREADS tables, ten
    /// voices an instance) and on a quad-core.
    #[test]
    fn the_count_follows_the_machine() {
        let mac = Cores { all: 14, fast: 10 };
        let linux = Cores { all: 16, fast: 16 };
        let windows = Cores { all: 20, fast: 6 };
        let quad = Cores { all: 4, fast: 4 };
        assert_eq!(
            [
                cap_for(mac),
                cap_for(linux),
                cap_for(windows),
                cap_for(quad)
            ],
            [8, 14, 4, 2]
        );
        // One instance: the Mac 8 (B 11, C 8), Linux 9 (the voices less one), Windows 4 (C).
        assert_eq!(budget_for_macos(14, 1), 11);
        assert_eq!(holds(11, 1, 8, 10), [8]);
        assert_eq!(budget_for(16, 1), 13);
        assert_eq!(holds(13, 1, 14, 10), [9]);
        assert_eq!(budget_for(20, 1), 17);
        assert_eq!(holds(17, 1, 4, 10), [4]);
        // Two: the Mac 5 + 5, Linux 6 + 6, Windows 4 + 4.
        assert_eq!(holds(budget_for_macos(14, 2), 2, 8, 10), [5, 5]);
        assert_eq!(holds(budget_for(16, 2), 2, 14, 10), [6, 6]);
        assert_eq!(holds(budget_for(20, 2), 2, 4, 10), [4, 4]);
        // Four: the Mac 2 each, Linux 3, 3, 2, 2, Windows 4, 4, 3, 3.
        assert_eq!(holds(budget_for_macos(14, 4), 4, 8, 10), [2, 2, 2, 2]);
        assert_eq!(holds(budget_for(16, 4), 4, 14, 10), [3, 3, 2, 2]);
        assert_eq!(holds(budget_for(20, 4), 4, 4, 10), [4, 4, 3, 3]);
        // Eight on the Mac: every instance one and two over (the macOS rule).
        assert_eq!(budget_for_macos(14, 8), 10);
        assert_eq!(budget_for(14, 8), 4);
        // Many instances: never below the floor; never more than N - 2 on the Mac.
        assert_eq!(budget_for(16, 40), 4);
        assert_eq!(budget_for_macos(14, 40), 12);
        // A quad-core: one worker, whoever asks.
        assert_eq!(budget_for(4, 1), 1);
        assert_eq!(holds(budget_for(4, 2), 2, 2, 10), [1, 0]);
    }

    /// The MC-79's UNISON on the Linux reference machine and the Windows machine: its voices
    /// less one bound it (UNISON 4, 3 workers; UNISON 2, 1), four instances of UNISON 4 three
    /// each on Linux (B 10) and on Windows (B 14, C 4).
    #[test]
    fn unisons_voices_bound_an_instance() {
        assert_eq!(holds(budget_for(16, 1), 1, 14, 4), [3]);
        assert_eq!(holds(budget_for(16, 1), 1, 14, 2), [1]);
        assert_eq!(holds(budget_for(16, 4), 4, 14, 4), [3, 3, 2, 2]);
        assert_eq!(holds(budget_for(20, 4), 4, 4, 4), [3, 3, 3, 3]);
    }

    /// Shares are even, the remainder to the first, and never more than the budget together.
    #[test]
    fn shares_are_even() {
        for most in 0..30 {
            for seats in 1..12 {
                let s: Vec<usize> = (0..seats).map(|r| share(most, seats, r)).collect();
                assert_eq!(s.iter().sum::<usize>(), most);
                assert!(s.windows(2).all(|w| w[0] >= w[1] && w[0] - w[1] <= 1));
            }
        }
        assert_eq!(share(5, 0, 0), 0);
    }
}
