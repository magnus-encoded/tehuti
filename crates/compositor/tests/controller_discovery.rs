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
    fn acquisition_never_returns_a_refused_controller() {
        assert!(
            controller::acquire(FakeDevice {
                refuse: true,
                ..FakeDevice::default()
            })
            .is_none()
        );
        let accepted = controller::acquire(FakeDevice::default()).expect("acquired");
        assert!(accepted.grabbed && accepted.nonblocking);
    }

    #[test]
    fn discovery_reconnects_only_after_a_due_scan_installs_a_controller() {
        let state = Discovery::Searching {
            retry_at: Duration::from_millis(250),
        };
        let (state, notice) = controller::scan_discovery(state, Duration::from_millis(249), false, || {
            panic!("scan before deadline")
        });
        assert_eq!(notice, None);
        let (state, notice) = controller::scan_discovery(state, Duration::from_millis(250), false, || false);
        assert_eq!(notice, None, "a refused or missing controller cannot reconnect");
        let (state, notice) = controller::scan_discovery(state, Duration::from_millis(251), true, || true);
        assert_eq!(notice, Some(DiscoveryEffect::Reconnected));
        let (_, notice) = controller::scan_discovery(state, Duration::from_secs(1), true, || true);
        assert_eq!(notice, None, "an attached controller cannot reconnect twice");
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

    #[test]
    fn attached_pads_do_not_block_periodic_or_directory_rescans() {
        for rescan in [false, true] {
            let (_, effect) = controller::discover(
                Discovery::Attached {
                    retry_at: Duration::from_millis(250),
                },
                DiscoveryInput::Poll {
                    now: Duration::from_millis(if rescan { 1 } else { 250 }),
                    rescan,
                },
            );
            assert_eq!(effect, Some(DiscoveryEffect::Scan));
        }
    }

    proptest! {
        #[test]
        fn reappearing_controller_is_found_within_one_retry_interval(reappears in 0_u64..10000, rescan in any::<bool>()) {
            let (mut state, _) = controller::discover(Discovery::Attached { retry_at: Duration::ZERO }, DiscoveryInput::Disconnected { now: Duration::ZERO });
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
            prop_assert_eq!(effect, Some(DiscoveryEffect::Scan));
        }

        #[test]
        fn reconnect_notification_requires_disconnect(sequence in prop::collection::vec(0_u8..4, 0..200)) {
            let mut state = Discovery::Attached { retry_at: Duration::ZERO };
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

                if effect == Some(DiscoveryEffect::Reconnected) {
                    prop_assert!(!attached, "duplicate device or reconnect without disconnect");
                    attached = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod hidraw_tests {
    use tehuti::controller::{HidrawSiblings, hidraw_siblings, hidraw_warning};

    fn hid_device(root: &std::path::Path, dir: &str) -> std::path::PathBuf {
        let hid = root.join(dir);
        std::fs::create_dir_all(&hid).expect("hid dir");
        std::fs::create_dir_all(root.join("bus/hid")).expect("bus dir");
        std::os::unix::fs::symlink(root.join("bus/hid"), hid.join("subsystem")).expect("subsystem link");
        hid
    }

    #[test]
    fn siblings_and_phys_come_from_the_hid_device_only() {
        let root = tempfile::tempdir().expect("tempdir");
        let parent = hid_device(root.path(), "devices/parent");
        std::fs::create_dir_all(parent.join("hidraw/hidraw1")).expect("parent hidraw");
        let hid = parent.join("child");
        std::fs::create_dir_all(&hid).expect("child dir");
        std::os::unix::fs::symlink(root.path().join("bus/hid"), hid.join("subsystem")).expect("link");
        let event = hid.join("input/input5/event5");
        std::fs::create_dir_all(&event).expect("event dir");
        std::fs::create_dir_all(hid.join("hidraw/hidraw3")).expect("hidraw dir");
        std::fs::write(hid.join("uevent"), "DRIVER=x\nHID_PHYS=pad/input0\n").expect("uevent");
        assert_eq!(
            hidraw_siblings(&event),
            HidrawSiblings {
                nodes: vec!["hidraw3".to_owned()],
                phys: Some("pad/input0".to_owned()),
            }
        );
    }

    #[test]
    fn a_parent_hidraw_is_not_a_sibling_without_a_hid_device() {
        let root = tempfile::tempdir().expect("tempdir");
        let parent = root.path().join("devices/usb");
        let event = parent.join("input/input5/event5");
        std::fs::create_dir_all(&event).expect("event dir");
        std::fs::create_dir_all(parent.join("hidraw/hidraw1")).expect("hidraw dir");
        assert_eq!(hidraw_siblings(&event), HidrawSiblings::default());
    }

    #[test]
    fn readable_sibling_warns_with_the_real_rule_and_closed_ones_stay_quiet() {
        let siblings = HidrawSiblings {
            nodes: vec!["hidraw3".to_owned(), "hidraw4".to_owned()],
            phys: Some("pad/input0".to_owned()),
        };
        let message = hidraw_warning("Pad", &siblings, |node| node == "hidraw4").expect("warns");
        assert!(message.contains("/dev/hidraw4"));
        assert!(!message.contains("/dev/hidraw3"));
        assert!(message.contains("ENV{HID_PHYS}==\"pad/input0\""));
        assert!(message.contains("TAG-=\"uaccess\""));
        assert!(hidraw_warning("Pad", &siblings, |_| false).is_none());
    }
}
