//! Frame failures go through the production recovery and loop driver with a fake submit.

#[cfg(test)]
mod tests {
    use core::{cell::RefCell, error::Error, time::Duration};
    use std::{rc::Rc, time::Instant};

    use proptest::prelude::*;
    use smithay::reexports::calloop::{
        EventLoop,
        timer::{TimeoutAction, Timer},
    };
    use tehuti::frame_recovery::{
        Driver, FAILURES_BEFORE_BACKOFF, FAILURES_BEFORE_FALLBACK, Frames, RETRY_INTERVAL, Recovery,
        STALLED_RETRY_INTERVAL,
    };

    /// An output whose submit fails a set number of times. Like the DRM output it
    /// draws only with new damage, unless the recovery retries a frame.
    #[derive(Default)]
    struct Output {
        recovery: Recovery,
        damaged: bool,
        failures_left: u32,
        attempts: u32,
        presented: u32,
        fallbacks: u32,
        /// Renders that fail outside the recovery, like a lost flip event.
        errors_left: u32,
    }

    impl Frames<()> for Output {
        fn render(&mut self, _data: &mut (), _tick: bool) -> Result<Option<Instant>, Box<dyn Error>> {
            if self.errors_left > 0 {
                self.errors_left = self.errors_left.saturating_sub(1);
                self.attempts = self.attempts.saturating_add(1);
                return Err("draw failed".into());
            }
            let now = Instant::now();
            if self.recovery.waiting(now) || !(self.damaged || self.recovery.retrying()) {
                return Ok(None);
            }
            self.damaged = false;
            self.attempts = self.attempts.saturating_add(1);
            let submitted = if self.failures_left == 0 {
                self.presented = self.presented.saturating_add(1);
                Ok(())
            } else {
                self.failures_left = self.failures_left.saturating_sub(1);
                Err("page flip failed")
            };
            let fallbacks = &mut self.fallbacks;
            Ok(self
                .recovery
                .finish(now, submitted, || *fallbacks = fallbacks.saturating_add(1)))
        }
    }

    /// Damages the output once, then runs the loop with no other source until it
    /// stops or `limit` passes.
    fn run(failures: u32, limit: Duration) -> (Output, Duration, bool) {
        run_output(
            Output {
                damaged: true,
                failures_left: failures,
                ..Output::default()
            },
            limit,
        )
    }

    fn run_output(output: Output, limit: Duration) -> (Output, Duration, bool) {
        let mut event_loop: EventLoop<'static, ()> = EventLoop::try_new().unwrap();
        let output = Rc::new(RefCell::new(output));
        let handle = Rc::new(event_loop.handle());
        let driver = Driver::new(&handle, &output);
        let timed_out = Rc::new(RefCell::new(false));
        let flag = timed_out.clone();
        let stop = event_loop.get_signal();
        let _guard = event_loop
            .handle()
            .insert_source(Timer::from_duration(limit), move |_, (), ()| {
                *flag.borrow_mut() = true;
                stop.stop();
                TimeoutAction::Drop
            })
            .unwrap();
        let started = Instant::now();
        driver.render(&mut (), false);
        event_loop.run(None, &mut (), |()| {}).unwrap();
        let elapsed = started.elapsed();
        drop(driver);
        let timed_out = *timed_out.borrow();
        (
            Rc::try_unwrap(output).ok().unwrap().into_inner(),
            elapsed,
            timed_out,
        )
    }

    #[test]
    fn a_persistent_failure_keeps_retrying_slower_and_never_stops_the_loop() {
        let fast = RETRY_INTERVAL.saturating_mul(FAILURES_BEFORE_BACKOFF);
        let limit = fast.saturating_add(STALLED_RETRY_INTERVAL.saturating_mul(2));
        let (output, _, timed_out) = run(u32::MAX, limit);
        assert!(timed_out, "a failing output must not end the session");
        assert_eq!(output.fallbacks, 1);
        assert!(
            output.attempts > FAILURES_BEFORE_BACKOFF,
            "retries stalled without an outside wakeup: {}",
            output.attempts
        );
        assert!(
            output.attempts <= FAILURES_BEFORE_BACKOFF.saturating_add(3),
            "past the backoff threshold retries slow down: {}",
            output.attempts
        );
    }

    #[test]
    fn a_render_error_outside_the_recovery_is_retried_and_does_not_stop_the_loop() {
        let limit = STALLED_RETRY_INTERVAL.saturating_mul(3);
        let (output, _, timed_out) = run_output(
            Output {
                damaged: true,
                errors_left: 2,
                ..Output::default()
            },
            limit,
        );
        assert!(timed_out, "a render error must not end the session");
        assert_eq!((output.attempts, output.presented), (3, 1));
    }

    #[test]
    fn a_pending_retry_timer_keeps_nothing_alive_after_shutdown() {
        let mut event_loop: EventLoop<'static, ()> = EventLoop::try_new().unwrap();
        let output = Rc::new(RefCell::new(Output {
            damaged: true,
            failures_left: u32::MAX,
            ..Output::default()
        }));
        let handle = Rc::new(event_loop.handle());
        let driver = Driver::new(&handle, &output);
        driver.render(&mut (), false);
        let weak_output = Rc::downgrade(&output);
        let weak_handle = Rc::downgrade(&handle);
        drop((driver, output, handle));
        assert!(
            weak_output.upgrade().is_none(),
            "the timer must not own the output"
        );
        assert!(
            weak_handle.upgrade().is_none(),
            "the timer must not own the loop handle"
        );
        event_loop
            .dispatch(Some(RETRY_INTERVAL.saturating_mul(3)), &mut ())
            .unwrap();
        drop(event_loop);
    }

    #[test]
    fn a_recovered_failure_presents_the_frame_without_client_activity() {
        let (output, _, timed_out) = run(1, RETRY_INTERVAL.saturating_mul(10));
        assert!(timed_out, "a recovered output keeps the session running");
        assert_eq!((output.attempts, output.presented), (2, 1));
        assert!(!output.recovery.retrying());
    }

    proptest! {
        #[test]
        fn the_fallback_runs_once_at_the_first_failure_past_its_threshold(
            outcomes in prop::collection::vec(any::<bool>(), 0..200),
        ) {
            let mut recovery = Recovery::default();
            let mut streak = 0_u32;
            let now = Instant::now();
            let retry = now.checked_add(RETRY_INTERVAL).unwrap();
            let stalled = now.checked_add(STALLED_RETRY_INTERVAL).unwrap();
            for failed in outcomes {
                let result = if failed { Err("busy") } else { Ok(()) };
                let mut fell_back = false;
                let finished = recovery.finish(now, result, || fell_back = true);
                streak = if failed { streak.saturating_add(1) } else { 0 };
                prop_assert_eq!(fell_back, streak == FAILURES_BEFORE_FALLBACK);
                prop_assert_eq!(recovery.fallback_attempted(), streak >= FAILURES_BEFORE_FALLBACK);
                let deadline = if streak >= FAILURES_BEFORE_BACKOFF { stalled } else { retry };
                prop_assert_eq!(finished, failed.then_some(deadline));
                prop_assert_eq!(recovery.waiting(now), failed);
                prop_assert!(!recovery.waiting(deadline));
            }
        }
    }

    #[test]
    fn a_switch_waits_for_the_pending_flip_and_restores_the_confirmed_mode() {
        use tehuti::frame_recovery::{switch_or_restore, when_idle};

        struct FakeBackend {
            mode: &'static str,
            pending: bool,
            submissions: usize,
        }
        let mut backend = FakeBackend {
            mode: "1920x1080@120",
            pending: true,
            submissions: 0,
        };
        let mut request = Some("1920x1080@60");
        assert_eq!(when_idle(backend.pending, || request.take()), None);
        assert_eq!(request, Some("1920x1080@60"));
        assert_eq!(backend.submissions, 0);

        backend.pending = false;
        let requested = when_idle(backend.pending, || request.take()).unwrap();
        let result = switch_or_restore(
            &mut backend,
            "1920x1080@120",
            |backend| {
                assert!(!backend.pending, "a second commit would return EBUSY");
                backend.submissions += 1;
                backend.mode = requested;
                Err("EINVAL")
            },
            |backend, confirmed| backend.mode = confirmed,
        );
        assert_eq!(result, Err("EINVAL"));
        assert_eq!(backend.mode, "1920x1080@120");
        assert_eq!(backend.submissions, 1);
        assert_eq!(request, None);
    }

    #[test]
    fn a_deferred_switch_submits_after_the_flip_completes() {
        use tehuti::frame_recovery::{switch_or_restore, when_idle};
        let mut request = Some("1920x1080@60");
        let mut backend = ("1920x1080@120", 0);
        assert_eq!(when_idle(true, || request.take()), None);
        assert_eq!(backend, ("1920x1080@120", 0));
        let requested = when_idle(false, || request.take()).unwrap();
        let result = switch_or_restore(
            &mut backend,
            "1920x1080@120",
            |backend| {
                *backend = (requested, 1);
                Ok::<_, &str>(())
            },
            |backend, confirmed| backend.0 = confirmed,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(backend, ("1920x1080@60", 1));
        assert_eq!(request, None);
    }
}
