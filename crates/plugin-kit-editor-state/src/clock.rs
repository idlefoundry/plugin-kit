//! One deadline clock, from both released editors (CA-72 R48 and CA-74 R-ULTRA).
use std::time::{Duration, Instant};

/// Both editors round their thirtieth of a second to the nearest nanosecond.
pub const FRAME_INTERVAL: Duration = Duration::from_nanos(33_333_333);
pub const MAX_ADVANCE: Duration = Duration::from_millis(50);

/// The adapter may restore a timeline when restoring its scene. All moving parts use one tick.
#[derive(Clone, Debug, Default)]
pub struct FrameClock {
    pub last: Option<Instant>,
    pub due: Option<Instant>,
    moving: bool,
}
impl FrameClock {
    pub fn new() -> Self {
        Self::default()
    }
    /// Preserve average pacing through irregular callbacks. A late deadline advances once,
    /// capped at fifty milliseconds; it cannot accumulate a catch-up burst.
    pub fn tick(&mut self, now: Instant, moving: bool) -> Option<Duration> {
        if !moving {
            self.moving = false;
            return None;
        }
        if self.due.is_some_and(|due| now < due) {
            return None;
        }
        self.due = Some(
            self.due
                .map_or(now + FRAME_INTERVAL, |due| (due + FRAME_INTERVAL).max(now)),
        );
        let elapsed = if self.moving {
            self.last.map_or(Duration::ZERO, |last| {
                now.saturating_duration_since(last).min(MAX_ADVANCE)
            })
        } else {
            Duration::ZERO
        };
        self.last = Some(now);
        self.moving = true;
        Some(elapsed)
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifteen_millisecond_callbacks_keep_thirty_ticks_a_second() {
        let start = Instant::now();
        let mut clock = FrameClock::new();
        let mut count = 0;
        for n in 0..=200 {
            if clock
                .tick(start + Duration::from_millis(n * 15), true)
                .is_some()
            {
                count += 1;
            }
        }
        assert_eq!(count, 91);
    }
    #[test]
    fn stalls_advance_once_and_rest_preserves_the_deadline() {
        let start = Instant::now();
        let mut clock = FrameClock::new();
        assert_eq!(clock.tick(start, true), Some(Duration::ZERO));
        assert_eq!(clock.tick(start + Duration::from_millis(15), true), None);
        let late = start + Duration::from_secs(10);
        assert_eq!(clock.tick(late, true), Some(MAX_ADVANCE));
        assert_eq!(clock.tick(late, true), Some(Duration::ZERO));
        assert_eq!(clock.tick(late, true), None);
        assert_eq!(clock.tick(late, false), None);
        assert_eq!(clock.tick(late + Duration::from_millis(15), true), None);
        assert_eq!(
            clock.tick(late + Duration::from_millis(45), true),
            Some(Duration::ZERO)
        );
        clock.reset();
        assert_eq!(clock.tick(start, true), Some(Duration::ZERO));
    }
}
