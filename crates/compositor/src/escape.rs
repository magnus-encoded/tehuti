//! The escape thread: restarts Tehuti when the main loop cannot.
//!
//! It runs beside calloop and reads no compositor state. Twice a pad-only owner
//! was locked out by a loop that spun but swallowed input, so this thread
//! polls the Guide key state on its own ungrabbed open of the pad and watches a
//! heartbeat the main loop bumps. Either a 5 s Guide hold or 10 s without a
//! heartbeat exits the process with [`RESTART_STATUS`].

use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use std::sync::{Arc, Mutex, PoisonError};

use evdev::{Device, KeyCode};
use tracing::error;

/// Exit status asking the session to restart Tehuti.
pub const RESTART_STATUS: i32 = 22;
/// How long Guide must stay down to restart; longer than the 3 s menu hold.
pub const GUIDE_LIMIT: Duration = Duration::from_secs(5);
/// How long the heartbeat may stand still before restarting, once it has started.
pub const HEARTBEAT_LIMIT: Duration = Duration::from_secs(10);
/// How often the escape thread samples.
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long a normal exit may take before the thread forces `_exit`.
pub const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Why the escape thread restarts Tehuti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Guide was held for [`GUIDE_LIMIT`].
    GuideHeld,
    /// The heartbeat stood still for [`HEARTBEAT_LIMIT`].
    Stalled,
}

/// One observation by the escape thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Time since the thread started.
    pub now: Duration,
    /// Whether Guide is down right now.
    pub guide_down: bool,
    /// The main loop's heartbeat counter.
    pub beat: u64,
}

/// What the escape thread remembers between samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Watch {
    guide_since: Option<Duration>,
    beat: u64,
    beat_at: Duration,
}

/// Advances the watch by one sample and says whether to restart.
#[must_use]
pub fn step(watch: Watch, sample: Sample) -> (Watch, Option<Reason>) {
    let guide_since = sample.guide_down.then(|| watch.guide_since.unwrap_or(sample.now));
    let (beat, beat_at) = if sample.beat == watch.beat {
        (watch.beat, watch.beat_at)
    } else {
        (sample.beat, sample.now)
    };
    let next = Watch {
        guide_since,
        beat,
        beat_at,
    };
    let reason = if guide_since.is_some_and(|since| sample.now.saturating_sub(since) >= GUIDE_LIMIT) {
        Some(Reason::GuideHeld)
    } else if beat > 0 && sample.now.saturating_sub(beat_at) >= HEARTBEAT_LIMIT {
        Some(Reason::Stalled)
    } else {
        None
    };
    (next, reason)
}

/// The main loop's side of the escape thread.
#[derive(Debug, Clone, Default)]
pub struct Escape {
    heartbeat: Arc<AtomicU64>,
    pad: Arc<Mutex<Option<Device>>>,
}

impl Escape {
    /// Starts the escape thread.
    /// # Errors
    /// The thread could not be spawned.
    pub fn spawn() -> std::io::Result<Self> {
        let escape = Self::default();
        let watched = escape.clone();
        let _thread = std::thread::Builder::new()
            .name("tehuti-escape".into())
            .spawn(move || watched.watch())?;
        Ok(escape)
    }

    /// Records that the main loop made progress.
    pub fn beat(&self) {
        let _previous = self.heartbeat.fetch_add(1, Ordering::Relaxed);
    }

    /// Hands the escape thread an ungrabbed handle on the pad, or `None` when it is dropped.
    /// It must not share the grabbed handle: `EVIOCGKEY` flushes that client's queued key events.
    pub fn set_pad(&self, pad: Option<Device>) {
        *self.pad.lock().unwrap_or_else(PoisonError::into_inner) = pad;
    }

    fn guide_down(&self) -> bool {
        self.pad
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|pad| pad.get_key_state().ok())
            .is_some_and(|keys| keys.contains(KeyCode::BTN_MODE))
    }

    fn watch(&self) {
        let start = std::time::Instant::now();
        let mut watch = Watch::default();
        loop {
            std::thread::sleep(POLL_INTERVAL);
            let sample = Sample {
                now: start.elapsed(),
                guide_down: self.guide_down(),
                beat: self.heartbeat.load(Ordering::Relaxed),
            };
            let (next, reason) = step(watch, sample);
            watch = next;
            if let Some(reason) = reason {
                restart(reason);
            }
        }
    }
}

/// Exits with [`RESTART_STATUS`], forcing `_exit` if a normal exit hangs.
///
/// Exiting closes the DRM fd and the libseat session, which drops DRM master
/// and lets logind return the VT; the normal exit is the best-effort restore.
fn restart(reason: Reason) -> ! {
    error!(?reason, status = RESTART_STATUS, "escape: restarting Tehuti");
    #[expect(clippy::exit, reason = "the escape exists to end a wedged process")]
    let _exiter = std::thread::Builder::new()
        .name("tehuti-escape-exit".into())
        .spawn(|| std::process::exit(RESTART_STATUS));
    std::thread::sleep(EXIT_GRACE);
    #[expect(
        unsafe_code,
        reason = "_exit is the only exit that cannot block on a wedged thread"
    )]
    // SAFETY: _exit takes no pointers and never returns; skipping destructors is the point.
    unsafe {
        libc::_exit(RESTART_STATUS)
    }
}

#[cfg(test)]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "test clocks stay far below u64 overflow"
)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn run(samples: &[Sample]) -> Option<(usize, Reason)> {
        let mut watch = Watch::default();
        for (index, sample) in samples.iter().enumerate() {
            let (next, reason) = step(watch, *sample);
            watch = next;
            if let Some(reason) = reason {
                return Some((index, reason));
            }
        }
        None
    }

    fn millis(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn guide_held_five_seconds_restarts() {
        let samples: Vec<_> = (0..=100)
            .map(|tick| Sample {
                now: millis(tick * 50),
                guide_down: true,
                beat: tick,
            })
            .collect();
        assert_eq!(run(&samples), Some((100, Reason::GuideHeld)));
    }

    #[test]
    fn menu_hold_does_not_restart() {
        let samples: Vec<_> = (0..200)
            .map(|tick| Sample {
                now: millis(tick * 50),
                guide_down: tick % 80 < 70,
                beat: tick,
            })
            .collect();
        assert_eq!(run(&samples), None);
    }

    #[test]
    fn still_heartbeat_restarts_after_ten_seconds() {
        let samples: Vec<_> = (0..=200)
            .map(|tick| Sample {
                now: millis(tick * 50),
                guide_down: false,
                beat: 7,
            })
            .collect();
        assert_eq!(run(&samples), Some((200, Reason::Stalled)));
    }

    #[test]
    fn startup_before_first_beat_never_stalls() {
        let samples: Vec<_> = (0..1_000)
            .map(|tick| Sample {
                now: millis(tick * 50),
                guide_down: false,
                beat: 0,
            })
            .collect();
        assert_eq!(run(&samples), None);
    }

    proptest! {
        #[test]
        fn beating_loop_without_long_hold_never_restarts(
            steps in proptest::collection::vec((1_u64..500, any::<bool>()), 1..400),
        ) {
            let mut now = 0;
            let mut held = 0;
            let mut samples = Vec::new();
            for (beat, (gap, down)) in (1_u64..).zip(steps) {
                now += gap;
                held = if down { held + gap } else { 0 };
                let down = down && held < 4_500;
                if !down { held = 0; }
                samples.push(Sample { now: millis(now), guide_down: down, beat });
            }
            prop_assert_eq!(run(&samples), None);
        }

        #[test]
        fn restart_never_fires_before_a_limit(
            steps in proptest::collection::vec((1_u64..2_000, any::<bool>(), any::<bool>()), 1..200),
        ) {
            let mut watch = Watch::default();
            let mut now = 0;
            let mut beat = 0;
            let mut down_since = None;
            let mut beat_at = 0;
            for (gap, down, bump) in steps {
                now += gap;
                if bump { beat += 1; beat_at = now; }
                down_since = if down { Some(down_since.unwrap_or(now)) } else { None };
                let (next, reason) = step(watch, Sample { now: millis(now), guide_down: down, beat });
                watch = next;
                let guide = down_since.is_some_and(|since| now - since >= 5_000);
                let stalled = beat > 0 && now - beat_at >= 10_000;
                let expected = if guide { Some(Reason::GuideHeld) } else if stalled { Some(Reason::Stalled) } else { None };
                prop_assert_eq!(reason, expected);
                if reason.is_some() { break; }
            }
        }
    }
}
