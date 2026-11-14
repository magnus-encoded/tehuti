//! The watchdog fires once the loop stops beating, and never while it beats.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::sync::mpsc;

    use proptest::prelude::*;
    use tehuti::watchdog::{FROZEN_AFTER, frozen, spawn};

    proptest! {
        #[test]
        fn a_loop_is_frozen_exactly_when_its_last_beat_is_a_timeout_old(
            beats in prop::collection::vec(0_u64..20_000, 1..40),
            check_after in 0_u64..40_000,
        ) {
            let mut last = Duration::ZERO;
            for gap in beats {
                let now = last.saturating_add(Duration::from_millis(gap));
                let gap_frozen = Duration::from_millis(gap) >= FROZEN_AFTER;
                prop_assert_eq!(frozen(last, now, FROZEN_AFTER), gap_frozen);
                last = now;
            }
            let now = last.saturating_add(Duration::from_millis(check_after));
            prop_assert_eq!(frozen(last, now, FROZEN_AFTER), Duration::from_millis(check_after) >= FROZEN_AFTER);
            prop_assert!(!frozen(now, now, FROZEN_AFTER));
        }
    }

    const TIMEOUT: Duration = Duration::from_millis(200);

    #[test]
    fn a_beating_loop_is_left_alone_and_a_stopped_one_is_ended() {
        let (fired, freeze) = mpsc::channel();
        let heartbeat = spawn(TIMEOUT, move || fired.send(()).unwrap()).unwrap();
        for _ in 0..30 {
            heartbeat.beat();
            std::thread::sleep(TIMEOUT.checked_div(10).unwrap());
        }
        assert!(freeze.try_recv().is_err(), "fired while the loop was turning");
        assert!(
            freeze.recv_timeout(TIMEOUT.saturating_mul(5)).is_ok(),
            "did not fire after the loop stopped"
        );
        drop(heartbeat);
    }

    #[test]
    fn dropping_the_heartbeat_stops_the_watcher() {
        let (fired, freeze) = mpsc::channel();
        let heartbeat = spawn(TIMEOUT, move || fired.send(()).unwrap()).unwrap();
        drop(heartbeat);
        assert!(
            freeze.recv_timeout(TIMEOUT.saturating_mul(3)).is_err(),
            "fired after the loop ended on purpose"
        );
    }
}
