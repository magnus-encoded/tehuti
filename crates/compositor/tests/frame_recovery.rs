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
}
