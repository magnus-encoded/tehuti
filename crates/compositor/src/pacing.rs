//! When the output redraws and when periodic loop work runs.
//!
//! Frames are drawn only when the damage tracker finds a change, so an idle
//! output stops flipping and stops waking the loop on every vblank.

use core::time::Duration;
use std::time::Instant;

/// The buffer age the damage tracker needs for a scanout buffer.
///
/// `drawn` counts the frames drawn so far, and `buffer_drawn_at` is that count
/// when this buffer was last drawn. Age 0 asks for a full redraw: the buffer was
/// never drawn, or `full` forces one, as after a modeset.
#[must_use]
pub fn buffer_age(drawn: u64, buffer_drawn_at: Option<u64>, full: bool) -> usize {
    if full {
        return 0;
    }
    buffer_drawn_at
        .and_then(|earlier| drawn.checked_sub(earlier))
        .and_then(|age| usize::try_from(age).ok())
        .unwrap_or(0)
}

/// Allows work at most once per period, however often the loop wakes.
#[derive(Debug, Clone)]
pub struct Interval {
    period: Duration,
    last: Option<Instant>,
}

impl Interval {
    #[must_use]
    pub const fn new(period: Duration) -> Self {
        Self { period, last: None }
    }

    /// Whether the work is due at `now`. A due answer starts the next period.
    pub fn due(&mut self, now: Instant) -> bool {
        let due = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= self.period);
        if due {
            self.last = Some(now);
        }
        due
    }

    /// Starts the next period at `now`, as when the work just ran for another reason.
    pub const fn restart(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// The longest the event loop sleeps without an event.
pub const LOOP_TIMEOUT: Duration = Duration::from_millis(10);

/// How often the loop's periodic work runs: time, stick sampling and controller discovery.
/// It is a little under [`LOOP_TIMEOUT`], so a wakeup at the timeout always runs it.
pub const LOOP_WORK_PERIOD: Duration = Duration::from_millis(8);

/// How often the frame counters are logged at debug level.
pub const STATISTICS_PERIOD: Duration = Duration::from_secs(10);

/// The shortest gap between frame callbacks sent without a new frame.
pub const IDLE_FRAME_CALLBACK: Duration = Duration::from_millis(16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_never_drawn_is_redrawn_in_full() {
        assert_eq!(
            buffer_age(7, None, false),
            0,
            "unknown contents need a full redraw"
        );
    }

    #[test]
    fn a_forced_redraw_is_full_whatever_the_buffer_holds() {
        assert_eq!(buffer_age(7, Some(5), true), 0, "a modeset redraws everything");
    }

    #[test]
    fn alternating_double_buffers_are_two_frames_old() {
        let mut buffers: [Option<u64>; 2] = [None, None];
        let mut ages = Vec::new();
        for drawn in 0..6_u64 {
            let buffer = &mut buffers[usize::from(drawn % 2 == 1)];
            ages.push(buffer_age(drawn, *buffer, false));
            *buffer = Some(drawn);
        }
        assert_eq!(
            ages,
            [0, 0, 2, 2, 2, 2],
            "each buffer last held the frame before the previous one"
        );
    }

    #[test]
    fn a_buffer_drawn_again_without_a_flip_is_one_frame_old() {
        assert_eq!(buffer_age(4, Some(3), false), 1, "it holds the previous frame");
    }

    #[test]
    fn the_first_call_is_due() {
        let mut interval = Interval::new(Duration::from_millis(10));
        assert!(interval.due(Instant::now()), "nothing has run yet");
    }

    #[test]
    fn calls_inside_the_period_are_not_due() {
        let start = Instant::now();
        let mut interval = Interval::new(Duration::from_millis(10));
        assert!(interval.due(start), "first call runs");
        assert!(
            !interval.due(start + Duration::from_millis(4)),
            "a wakeup 4 ms later waits"
        );
        assert!(
            !interval.due(start + Duration::from_millis(9)),
            "a wakeup 9 ms later waits"
        );
        assert!(
            interval.due(start + Duration::from_millis(10)),
            "a full period later runs"
        );
    }

    #[test]
    fn fast_wakeups_run_the_work_once_per_period() {
        let start = Instant::now();
        let mut interval = Interval::new(Duration::from_millis(10));
        let runs = (0..1000_u64)
            .filter(|step| interval.due(start + Duration::from_millis(*step)))
            .count();
        assert_eq!(runs, 100, "1000 one-millisecond wakeups hold 100 periods");
    }
}
