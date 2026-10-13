//! Controller acquisition and discovery without kernel devices.
#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::io;

    use proptest::prelude::*;
    use tehuti::controller::{self, Discovery, DiscoveryEffect, DiscoveryInput, GrabDevice};

    #[derive(Default)]
    struct FakeDevice {
        grabbed: bool,
        nonblocking: bool,
        refuse: bool,
    }

    impl GrabDevice for FakeDevice {
        fn grab(&mut self) -> io::Result<()> {
            if self.refuse {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.grabbed = true;
            Ok(())
        }
        fn set_nonblocking(&mut self, enabled: bool) -> io::Result<()> {
            self.nonblocking = enabled;
            Ok(())
        }
    }

    #[test]
    fn acquisition_grabs_exclusively_and_reports_refusal() {
        let mut device = FakeDevice::default();
        controller::grab_nonblocking(&mut device).expect("device accepts grab");
        assert!(device.grabbed);
        assert!(device.nonblocking);
        let mut refused = FakeDevice {
            refuse: true,
            ..FakeDevice::default()
        };
        assert_eq!(
            controller::grab_nonblocking(&mut refused)
                .expect_err("grab refused")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(!refused.nonblocking);
    }

    proptest! {
        #[test]
        fn reappearing_controller_is_found_within_one_retry_interval(reappears in 0_u64..10000, rescan in any::<bool>()) {
            let (mut state, _) = controller::discover(Discovery::Attached, DiscoveryInput::Disconnected { now: Duration::ZERO });
            let mut picked_up = None;
            for millis in 0..=reappears.saturating_add(250) {
                let (next, effect) = controller::discover(state, DiscoveryInput::Poll { now: Duration::from_millis(millis), rescan: rescan && millis == reappears });
                state = next;
                if effect == Some(DiscoveryEffect::Scan) && millis >= reappears {
                    let (attached, notification) = controller::discover(state, DiscoveryInput::Found);
                    state = attached;
                    prop_assert_eq!(notification, Some(DiscoveryEffect::Reconnected));
                    picked_up = Some(millis);
                    break;
                }
            }
            prop_assert!(picked_up.is_some_and(|time| time <= reappears.saturating_add(250)));
            let (_, effect) = controller::discover(state, DiscoveryInput::Poll { now: Duration::MAX, rescan: true });
            prop_assert_eq!(effect, None);
        }

        #[test]
        fn reconnect_requires_disconnect_and_never_duplicates_devices(sequence in prop::collection::vec(0_u8..4, 0..200)) {
            let mut state = Discovery::Attached;
            let mut attached = true;
            for (time, kind) in sequence.into_iter().enumerate() {
                let input = match kind {
                    0 => DiscoveryInput::Disconnected { now: Duration::from_secs(u64::try_from(time).expect("bounded sequence")) },
                    1 => DiscoveryInput::Found,
                    _ => DiscoveryInput::Poll { now: Duration::from_secs(u64::try_from(time).expect("bounded sequence")), rescan: kind == 3 },
                };
                let (next, effect) = controller::discover(state, input);
                state = next;
                if kind == 0 { attached = false; }
                if effect == Some(DiscoveryEffect::Scan) { prop_assert!(!attached); }
                if effect == Some(DiscoveryEffect::Reconnected) {
                    prop_assert!(!attached, "duplicate device or reconnect without disconnect");
                    attached = true;
                }
            }
        }
    }
}
