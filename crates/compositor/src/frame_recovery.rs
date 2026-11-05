//! A failed frame is retried before persistent failure ends the session.

use core::{fmt::Display, time::Duration};

pub const MAX_CONSECUTIVE_FAILURES: u32 = 120;
pub const RETRY_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
pub struct Recovery {
    failures: u32,
}

impl Recovery {
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
