//! A failed frame is retried before persistent failure ends the session.

use core::{cell::RefCell, fmt::Display, time::Duration};
use std::{
    error::Error,
    rc::{Rc, Weak},
    time::Instant,
};

use smithay::reexports::calloop::{
    LoopSignal, WeakLoopHandle,
    timer::{TimeoutAction, Timer},
};

pub const FAILURES_BEFORE_FALLBACK: u32 = 20;
pub const FAILURES_AFTER_FALLBACK: u32 = 20;
pub const MAX_CONSECUTIVE_FAILURES: u32 = FAILURES_BEFORE_FALLBACK + FAILURES_AFTER_FALLBACK;
pub const RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// The failure streak of one output and the deadline of its next attempt.
#[derive(Debug, Default)]
pub struct Recovery {
    failures: u32,
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
    /// `FAILURES_BEFORE_FALLBACK`.
    ///
    /// # Errors
    /// Returns the last error after `MAX_CONSECUTIVE_FAILURES` failed frames.
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
        self.failures = self.failures.saturating_add(1);
        if self.failures >= MAX_CONSECUTIVE_FAILURES {
            return Err(error);
        }
        if self.failures == 1 {
            tracing::warn!(%error, "DRM frame failed; retrying");
        }
        if self.failures >= FAILURES_BEFORE_FALLBACK && !self.fallback_attempted {
            self.fallback_attempted = true;
            fall_back();
        }
        let deadline = now.checked_add(RETRY_INTERVAL).unwrap_or(now);
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
    handle: WeakLoopHandle<'l, Data>,
    frames: Weak<RefCell<F>>,
    stop: LoopSignal,
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
    pub fn new(handle: WeakLoopHandle<'l, Data>, frames: &Rc<RefCell<F>>, stop: LoopSignal) -> Self {
        Self {
            handle,
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
        match result {
            Ok(None) => {}
            Ok(Some(deadline)) => {
                let driver = self.clone();
                let armed = self.handle.upgrade().map(|handle| {
                    handle.insert_source(Timer::from_deadline(deadline), move |_, (), data| {
                        driver.render(data, true);
                        TimeoutAction::Drop
                    })
                });
                if !matches!(armed, Some(Ok(_))) {
                    tracing::warn!("DRM frame retry timer failed; ending session");
                    self.stop.stop();
                }
            }
            Err(error) => {
                tracing::warn!(%error, "DRM frame failed; ending session");
                self.stop.stop();
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
