//! Ends a compositor whose main loop stops turning, so the session falls back.
//!
//! The loop beats a [`Heartbeat`] each turn. A watcher thread compares the last
//! beat with a monotonic clock, which stands still across system suspend.

use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use std::{
    sync::{Arc, Weak},
    time::Instant,
};

/// How long the loop may stay still before the compositor counts as frozen.
pub const FROZEN_AFTER: Duration = Duration::from_secs(15);

/// Whether a loop last seen at `last_beat` is frozen at `now`; both are times since one start.
#[must_use]
pub fn frozen(last_beat: Duration, now: Duration, timeout: Duration) -> bool {
    now.saturating_sub(last_beat) >= timeout
}

/// The loop's side: beat once a turn. Dropping it stops the watcher.
#[derive(Debug)]
pub struct Heartbeat {
    start: Instant,
    last_beat_ms: Arc<AtomicU64>,
}

impl Heartbeat {
    pub fn beat(&self) {
        self.last_beat_ms
            .store(millis(self.start.elapsed()), Ordering::Relaxed);
    }
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Starts a watcher that runs `on_freeze` once if no beat arrives for `timeout`.
///
/// # Errors
/// Returns the error of spawning the watcher thread.
pub fn spawn(timeout: Duration, on_freeze: impl FnOnce() + Send + 'static) -> std::io::Result<Heartbeat> {
    let start = Instant::now();
    let last_beat_ms = Arc::new(AtomicU64::new(0));
    let watched: Weak<AtomicU64> = Arc::downgrade(&last_beat_ms);
    let poll = timeout
        .checked_div(4)
        .unwrap_or(timeout)
        .max(Duration::from_millis(1));
    let _watcher = std::thread::Builder::new()
        .name("tehuti-watchdog".into())
        .spawn(move || {
            loop {
                std::thread::sleep(poll);
                let Some(last_beat) = watched.upgrade() else {
                    return;
                };
                let last = Duration::from_millis(last_beat.load(Ordering::Relaxed));
                if frozen(last, start.elapsed(), timeout) {
                    on_freeze();
                    return;
                }
            }
        })?;
    Ok(Heartbeat { start, last_beat_ms })
}

/// Logs the freeze and aborts; a second thread aborts anyway if logging blocks.
pub fn abort_frozen() {
    let _guard = std::thread::Builder::new().spawn(|| {
        std::thread::sleep(Duration::from_secs(1));
        std::process::abort();
    });
    tracing::error!(timeout = ?FROZEN_AFTER, "main loop stopped turning; aborting so the session falls back");
    std::process::abort();
}
