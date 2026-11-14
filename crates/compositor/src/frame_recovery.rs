//! A failed frame is retried; only a minute of nothing but failed frames ends the session.

use core::{cell::RefCell, error::Error, fmt::Display, time::Duration};
use std::{
    rc::{Rc, Weak},
    time::Instant,
};

use smithay::reexports::calloop::{
    LoopHandle, LoopSignal,
    timer::{TimeoutAction, Timer},
};

pub const FAILURES_BEFORE_FALLBACK: u32 = 20;
pub const FAILURES_AFTER_FALLBACK: u32 = 20;
/// Failures after which the output retries at [`STALLED_RETRY_INTERVAL`].
pub const FAILURES_BEFORE_BACKOFF: u32 = FAILURES_BEFORE_FALLBACK + FAILURES_AFTER_FALLBACK;
pub const RETRY_INTERVAL: Duration = Duration::from_millis(50);
/// The retry pace of an output that keeps failing.
pub const STALLED_RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// How long every frame may fail before the session exits, so its launcher
/// restarts Tehuti or falls back; a black screen with no way out is worse.
pub const FAILING_FRAMES_EXIT_AFTER: Duration = Duration::from_secs(60);

/// The failure streak of one output and the deadline of its next attempt.
#[derive(Debug, Default)]
pub struct Recovery {
    failures: u32,
    failing_since: Option<Instant>,
    fallback_attempted: bool,
    retry_at: Option<Instant>,
}

impl Recovery {
    #[must_use]
    pub const fn fallback_attempted(&self) -> bool {
        self.fallback_attempted
    }

    /// Whether a retry is scheduled after `now`, so this attempt must wait.
    #[must_use]
    pub fn waiting(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|deadline| now < deadline)
    }

    /// Whether a frame is being retried, which must not wait for new damage.
    #[must_use]
    pub const fn retrying(&self) -> bool {
        self.failures > 0
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Records a frame outcome and returns the deadline of the retry it needs.
    ///
    /// `fall_back` runs once per streak, at the first failure at or past
    /// `FAILURES_BEFORE_FALLBACK`. From `FAILURES_BEFORE_BACKOFF` on, retries
    /// wait [`STALLED_RETRY_INTERVAL`].
    ///
    /// # Errors
    /// Returns the error once every frame has failed for [`FAILING_FRAMES_EXIT_AFTER`].
    pub fn finish<E: Display>(
        &mut self,
        now: Instant,
        result: Result<(), E>,
        fall_back: impl FnOnce(),
    ) -> Result<Option<Instant>, E> {
        let Err(error) = result else {
            self.reset();
            return Ok(None);
        };
        let since = *self.failing_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= FAILING_FRAMES_EXIT_AFTER {
            return Err(error);
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures == 1 {
            tracing::warn!(%error, "DRM frame failed; retrying");
        }
        if self.failures == FAILURES_BEFORE_BACKOFF {
            tracing::error!(%error, "DRM frames keep failing; retrying every second");
        }
        if self.failures >= FAILURES_BEFORE_FALLBACK && !self.fallback_attempted {
            self.fallback_attempted = true;
            fall_back();
        }
        let interval = if self.failures >= FAILURES_BEFORE_BACKOFF {
            STALLED_RETRY_INTERVAL
        } else {
            RETRY_INTERVAL
        };
        let deadline = now.checked_add(interval).unwrap_or(now);
        self.retry_at = Some(deadline);
        Ok(Some(deadline))
    }
}

/// An output whose frames the event loop drives.
pub trait Frames<Data> {
    /// Draws a frame if one is due.
    ///
    /// # Errors
    /// Returns an error the session cannot continue after.
    fn render(&mut self, data: &mut Data, tick: bool) -> Result<Option<Instant>, Box<dyn Error>>;
}

/// Renders frames and wakes the loop at each retry deadline.
pub struct Driver<'l, Data, F> {
    /// Weak, so a pending retry timer does not keep its own loop alive.
    handle: Weak<LoopHandle<'l, Data>>,
    frames: Weak<RefCell<F>>,
    stop: LoopSignal,
}

impl<Data, F> core::fmt::Debug for Driver<'_, Data, F> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Driver").finish_non_exhaustive()
    }
}

impl<Data, F> Clone for Driver<'_, Data, F> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            frames: self.frames.clone(),
            stop: self.stop.clone(),
        }
    }
}

impl<'l, Data: 'l, F: Frames<Data> + 'l> Driver<'l, Data, F> {
    #[must_use]
    pub fn new(handle: &Rc<LoopHandle<'l, Data>>, frames: &Rc<RefCell<F>>, stop: LoopSignal) -> Self {
        Self {
            handle: Rc::downgrade(handle),
            frames: Rc::downgrade(frames),
            stop,
        }
    }

    /// Renders now; a failed frame arms a timer at its retry deadline.
    pub fn render(&self, data: &mut Data, tick: bool) {
        let Some(frames) = self.frames.upgrade() else {
            return;
        };
        let result = frames.borrow_mut().render(data, tick);
        let deadline = match result {
            Ok(None) => return,
            Ok(Some(deadline)) => deadline,
            Err(error) => {
                tracing::error!(%error, "DRM frames failed for a minute; ending session");
                self.stop.stop();
                return;
            }
        };
        let driver = self.clone();
        let armed = self.handle.upgrade().map(|handle| {
            handle.insert_source(Timer::from_deadline(deadline), move |_, (), loop_data| {
                driver.render(loop_data, true);
                TimeoutAction::Drop
            })
        });
        if matches!(armed, Some(Err(_))) {
            // The loop's periodic redraw retries the frame instead.
            tracing::warn!("DRM frame retry timer failed");
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
