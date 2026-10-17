//! Frame pacing: one flip in flight, released by its vblank or by a stall watchdog.

use core::time::Duration;

/// Refresh periods a submitted flip may wait for its vblank before it counts as lost.
const STALL_PERIODS: u32 = 8;
/// The floor on the stall window, so low refresh rates and slow drivers are not cut short.
const STALL_FLOOR: Duration = Duration::from_millis(250);

/// What the render tick may do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tick {
    /// No flip is in flight; draw and submit a frame.
    Render,
    /// A flip is in flight and still inside its window; wait for the vblank.
    Wait,
    /// A flip has waited past its window; its vblank is lost, so draw again.
    Stalled,
}

/// The submitted flip, measured on the loop clock that callers pass in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Flip {
    submitted: Option<Duration>,
}

impl Flip {
    pub(super) const fn submitted(&mut self, now: Duration) {
        self.submitted = Some(now);
    }

    pub(super) const fn released(&mut self) {
        self.submitted = None;
    }

    pub(super) fn tick(&self, now: Duration, refresh_hz: u32) -> Tick {
        let Some(submitted) = self.submitted else {
            return Tick::Render;
        };
        if now.saturating_sub(submitted) >= stall_window(refresh_hz) {
            Tick::Stalled
        } else {
            Tick::Wait
        }
    }
}

fn stall_window(refresh_hz: u32) -> Duration {
    Duration::from_secs(1)
        .checked_div(refresh_hz.max(1))
        .and_then(|period| period.checked_mul(STALL_PERIODS))
        .unwrap_or(STALL_FLOOR)
        .max(STALL_FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Clone, Debug)]
    enum Step {
        Advance(u64),
        Vblank,
        Tick,
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            (0_u64..400).prop_map(Step::Advance),
            Just(Step::Vblank),
            Just(Step::Tick),
        ]
    }

    #[test]
    fn a_lost_vblank_releases_the_frame_within_the_window() {
        let mut flip = Flip::default();
        flip.submitted(Duration::ZERO);
        assert_eq!(flip.tick(Duration::from_millis(16), 60), Tick::Wait);
        assert_eq!(flip.tick(Duration::from_millis(250), 60), Tick::Stalled);
        assert_eq!(flip.tick(Duration::from_millis(267), 30), Tick::Stalled);
        assert_eq!(flip.tick(Duration::from_millis(10), 0), Tick::Wait);
    }

    proptest! {
        /// Over any series of ticks, vblanks and elapsed time, the render tick
        /// never goes longer than the stall window without being allowed to draw:
        /// a lost flip cannot freeze the screen.
        #[test]
        fn rendering_never_waits_past_the_stall_window(
            steps in prop::collection::vec(step(), 0..200),
            refresh_hz in 0_u32..=240,
        ) {
            let mut flip = Flip::default();
            let mut now = Duration::ZERO;
            let window = stall_window(refresh_hz);
            for step in steps {
                match step {
                    Step::Advance(millis) => now = now.saturating_add(Duration::from_millis(millis)),
                    Step::Vblank => flip.released(),
                    Step::Tick => {
                        let since = flip.submitted.map(|submitted| now.saturating_sub(submitted));
                        let tick = flip.tick(now, refresh_hz);
                        match since {
                            None => prop_assert_eq!(tick, Tick::Render),
                            Some(waited) if waited >= window => prop_assert_eq!(tick, Tick::Stalled),
                            Some(_) => prop_assert_eq!(tick, Tick::Wait),
                        }
                        if tick != Tick::Wait {
                            flip.submitted(now);
                        }
                    }
                }
            }
        }
    }
}
