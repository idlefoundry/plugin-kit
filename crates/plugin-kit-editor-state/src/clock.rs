//! One clock for all an editor's moving parts, from the CA-72's frame loop (R48).

use std::time::{Duration, Instant};

/// Round upwards so two accepted ticks are never less than a thirtieth of a second apart.
pub const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000_u64.div_ceil(30));
/// A stalled frame advances animations by at most this much, without catching up later.
pub const MAX_ADVANCE: Duration = Duration::from_millis(50);

/// The caller supplies one time for all moving parts. No sleeps or platform timers live here.
#[derive(Clone, Debug, Default)]
pub struct FrameClock {
    last: Option<Instant>,
}

impl FrameClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// No tick at rest; the first moving tick advances by zero. Following ticks advance by
    /// actual time since the last accepted tick, capped at 50 ms and at most 30 times a second.
    /// Early or backwards times do not change the clock. Resuming after rest starts afresh.
    pub fn tick(&mut self, now: Instant, moving: bool) -> Option<Duration> {
        if !moving {
            self.reset();
            return None;
        }
        let elapsed = match self.last {
            Some(last) => {
                let elapsed = now.saturating_duration_since(last);
                if elapsed < FRAME_INTERVAL {
                    return None;
                }
                elapsed.min(MAX_ADVANCE)
            }
            None => Duration::ZERO,
        };
        self.last = Some(now);
        Some(elapsed)
    }

    /// Forget a previous timeline, for example when replacing an editor's scene.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacing_and_elapsed_time_use_the_same_clock() {
        let start = Instant::now();
        let mut clock = FrameClock::new();
        assert_eq!(clock.tick(start, true), Some(Duration::ZERO));
        assert_eq!(clock.tick(start, true), None);
        assert_eq!(
            clock.tick(start + FRAME_INTERVAL - Duration::from_nanos(1), true),
            None
        );
        assert_eq!(
            clock.tick(start + FRAME_INTERVAL, true),
            Some(FRAME_INTERVAL)
        );
        assert_eq!(clock.tick(start, true), None);
        assert_eq!(
            clock.tick(start + FRAME_INTERVAL * 2, true),
            Some(FRAME_INTERVAL)
        );
    }

    #[test]
    fn irregular_frames_never_catch_up_with_a_burst() {
        let mut clock = FrameClock::new();
        let start = Instant::now();
        let mut last = None;
        let mut accepted = 0;
        for ms in 0..=2000 {
            let now = start + Duration::from_millis(ms);
            if let Some(dt) = clock.tick(now, true) {
                if let Some(previous) = last {
                    assert!(now.duration_since(previous) >= FRAME_INTERVAL);
                    assert_eq!(dt, now.duration_since(previous));
                }
                last = Some(now);
                accepted += 1;
            }
        }
        assert!(accepted <= 61);
        let now = start + Duration::from_secs(20);
        assert_eq!(clock.tick(now, true), Some(MAX_ADVANCE));
        assert_eq!(clock.tick(now + Duration::from_millis(1), true), None);
        assert_eq!(clock.tick(now + FRAME_INTERVAL, true), Some(FRAME_INTERVAL));
    }

    #[test]
    fn a_stall_is_capped_and_rest_does_not_advance() {
        let start = Instant::now();
        let mut clock = FrameClock::new();
        assert_eq!(clock.tick(start, false), None);
        assert_eq!(clock.tick(start, true), Some(Duration::ZERO));
        assert_eq!(
            clock.tick(start + Duration::from_millis(45), true),
            Some(Duration::from_millis(45))
        );
        assert_eq!(
            clock.tick(start + Duration::from_secs(60), true),
            Some(MAX_ADVANCE)
        );
        assert_eq!(clock.tick(start + Duration::from_secs(61), false), None);
        assert_eq!(
            clock.tick(start + Duration::from_secs(120), true),
            Some(Duration::ZERO)
        );
        clock.reset();
        assert_eq!(clock.tick(start, true), Some(Duration::ZERO));
    }
}
