//! DRM notifications use the production handler without opening devices.

#[cfg(test)]
mod tests {
    use core::ops::ControlFlow;
    use proptest::prelude::*;
    use smithay::backend::udev::UdevEvent;
    use tehuti::drm_hotplug::handle_event;

    #[test]
    fn changed_rescans_connectors_and_keeps_the_session_running() {
        let mut rescanned = false;
        let session = handle_event(&UdevEvent::Changed { device_id: 7 }, 7, || {
            rescanned = true;
        });
        assert_eq!(session, ControlFlow::Continue(()), "hotplug keeps clients alive");
        assert!(rescanned, "hotplug must refresh connectors and modes");
    }
    #[test]
    fn removed_ends_the_session_without_rescanning() {
        let mut rescanned = false;
        let session = handle_event(&UdevEvent::Removed { device_id: 7 }, 7, || {
            rescanned = true;
        });
        assert_eq!(session, ControlFlow::Break(()), "device removal ends the session");
        assert!(!rescanned, "a removed device cannot be rescanned");
    }

    proptest! {
        #[test]
        fn notifications_for_other_devices_leave_the_session_and_outputs_alone(
            active in any::<libc::dev_t>(),
            other in any::<libc::dev_t>(),
        ) {
            prop_assume!(active != other);
            for event in [
                UdevEvent::Added { device_id: other, path: "/unused/card".into() },
                UdevEvent::Changed { device_id: other },
                UdevEvent::Removed { device_id: other },
            ] {
                let mut rescanned = false;
                let session = handle_event(&event, active, || rescanned = true);
                prop_assert_eq!(session, ControlFlow::Continue(()));
                prop_assert!(!rescanned);
            }
        }

        #[test]
        fn every_changed_device_rescans_without_ending_its_session(device_id in any::<libc::dev_t>()) {
            let mut rescanned = false;
            let session = handle_event(&UdevEvent::Changed { device_id }, device_id, || rescanned = true);
            prop_assert_eq!(session, ControlFlow::Continue(()));
            prop_assert!(rescanned);
        }
    }
}
