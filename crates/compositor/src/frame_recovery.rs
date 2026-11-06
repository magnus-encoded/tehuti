//! A failed frame is retried before persistent failure ends the session.

use core::{fmt::Display, time::Duration};

pub const FAILURES_BEFORE_FALLBACK: u32 = 20;
pub const FAILURES_AFTER_FALLBACK: u32 = 20;
pub const MAX_CONSECUTIVE_FAILURES: u32 = FAILURES_BEFORE_FALLBACK + FAILURES_AFTER_FALLBACK;
pub const RETRY_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
pub struct Recovery {
    failures: u32,
}

impl Recovery {
    #[must_use]
    pub const fn fallback_attempted(&self) -> bool {
        self.failures >= FAILURES_BEFORE_FALLBACK
    }

    #[must_use]
    pub const fn fallback_due(&self) -> bool {
        self.failures == FAILURES_BEFORE_FALLBACK
    }

    /// Returns a completed frame, a retry, or the persistent failure.
    ///
    /// # Errors
    /// Returns the last error after `MAX_CONSECUTIVE_FAILURES` failed frames.
    pub fn finish<T, E: Display>(&mut self, result: Result<T, E>) -> Result<Option<T>, E> {
        match result {
            Ok(frame) => {
                self.failures = 0;
                Ok(Some(frame))
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                if self.failures >= MAX_CONSECUTIVE_FAILURES {
                    return Err(error);
                }
                if self.failures == 1 {
                    tracing::warn!(%error, "DRM frame failed; retrying");
                }
                Ok(None)
            }
        }
    }
}

/// Leaves requests queued until scanout can accept another commit.
pub fn when_idle<T>(pending: bool, take: impl FnOnce() -> Option<T>) -> Option<T> {
    if pending { None } else { take() }
}

/// Restores the confirmed output when a requested mode cannot be submitted.
///
/// # Errors
/// Returns the switch error after attempting restoration.
pub fn switch_or_restore<State, Mode: Copy, Error>(
    state: &mut State,
    confirmed: Mode,
    switch: impl FnOnce(&mut State) -> Result<(), Error>,
    restore: impl FnOnce(&mut State, Mode),
) -> Result<(), Error> {
    switch(state).inspect_err(|_| restore(state, confirmed))
}
