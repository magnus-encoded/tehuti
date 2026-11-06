//! Frame failures use the production recovery policy without opening a device.

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tehuti::frame_recovery::{MAX_CONSECUTIVE_FAILURES, Recovery};

    struct Backend {
        failures_remaining: u32,
    }

    impl Backend {
        fn render(&mut self) -> Result<(), &'static str> {
            if self.failures_remaining == 0 {
                return Ok(());
            }
            self.failures_remaining = self.failures_remaining.saturating_sub(1);
            Err("busy")
        }
    }

    #[test]
    fn transient_backend_failures_keep_the_session_running() {
        for failures in [1, MAX_CONSECUTIVE_FAILURES - 1] {
            let mut recovery = Recovery::default();
            let mut backend = Backend {
                failures_remaining: failures,
            };
            for _ in 0..failures {
                assert_eq!(recovery.finish(backend.render()), Ok(None));
            }
            assert_eq!(recovery.finish(backend.render()), Ok(Some(())));
            backend.failures_remaining = failures;
            for _ in 0..failures {
                assert_eq!(recovery.finish(backend.render()), Ok(None));
            }
        }
    }

    #[test]
    fn persistent_backend_failure_ends_the_session_at_the_bound() {
        let mut recovery = Recovery::default();
        let mut backend = Backend {
            failures_remaining: u32::MAX,
        };
        for _ in 1..MAX_CONSECUTIVE_FAILURES {
            assert_eq!(recovery.finish(backend.render()), Ok(None));
        }
        assert_eq!(recovery.finish(backend.render()), Err("busy"));
    }

    proptest! {
        #[test]
        fn a_success_resets_any_failure_streak(failures in 0..MAX_CONSECUTIVE_FAILURES) {
            let mut recovery = Recovery::default();
            let mut backend = Backend { failures_remaining: failures };
            for _ in 0..failures {
                prop_assert_eq!(recovery.finish(backend.render()), Ok(None));
            }
            prop_assert_eq!(recovery.finish(backend.render()), Ok(Some(())));
            backend.failures_remaining = MAX_CONSECUTIVE_FAILURES;
            for _ in 1..MAX_CONSECUTIVE_FAILURES {
                prop_assert_eq!(recovery.finish(backend.render()), Ok(None));
            }
            prop_assert_eq!(recovery.finish(backend.render()), Err("busy"));
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

    #[test]
    fn persistent_failures_fall_back_before_the_session_ends() {
        use tehuti::frame_recovery::{FAILURES_AFTER_FALLBACK, FAILURES_BEFORE_FALLBACK, Recovery};
        let mut recovery = Recovery::default();
        let mut mode = "1920x1080@60";
        for _ in 1..FAILURES_BEFORE_FALLBACK {
            assert_eq!(recovery.finish(Err::<(), _>("busy")), Ok(None));
            assert!(!recovery.fallback_due());
        }
        assert_eq!(recovery.finish(Err::<(), _>("busy")), Ok(None));
        if recovery.fallback_due() {
            mode = "1920x1080@120";
        }
        assert_eq!(mode, "1920x1080@120");
        for _ in 1..FAILURES_AFTER_FALLBACK {
            assert_eq!(recovery.finish(Err::<(), _>("busy")), Ok(None));
            assert!(!recovery.fallback_due());
        }
        assert_eq!(recovery.finish(Err::<(), _>("busy")), Err("busy"));
        assert_eq!(recovery.finish(Ok::<_, &str>(())), Ok(Some(())));
        assert!(!recovery.fallback_due());
    }
}
