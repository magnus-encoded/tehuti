#[cfg(test)]
mod tests {
    use evdev::BusType;
    use tehuti::controller::{is_physical, virtual_pad_identity};

    #[test]
    fn virtual_pad_has_a_fixed_xbox_identity_and_is_not_physical() {
        let identity = virtual_pad_identity();
        assert_eq!(identity.bus_type(), BusType::BUS_VIRTUAL);
        assert_eq!(identity.vendor(), 0x045e);
        assert_eq!(identity.product(), 0x028e);
        assert_eq!(identity.version(), 0x0001);
        assert!(!is_physical(identity.bus_type(), None));
        assert!(!is_physical(
            identity.bus_type(),
            Some(std::path::Path::new("/sys/devices/virtual/input/input42/event20")),
        ));
    }

    #[test]
    fn virtual_pad_advertises_xbox_buttons_without_source_extras() {
        use evdev::KeyCode;
        let keys = tehuti::controller::virtual_pad_keys();
        assert_eq!(keys.iter().count(), 11);
        for key in [
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
        ] {
            assert!(keys.contains(key));
        }
        assert!(!keys.contains(KeyCode::KEY_RECORD));
        assert!(!keys.contains(KeyCode::BTN_TL2));
    }

    #[test]
    fn virtual_pad_axes_keep_calibration_and_exclude_source_extras() {
        use evdev::{AbsInfo, AbsoluteAxisCode as Axis};
        let source = [
            (Axis::ABS_X, AbsInfo::new(128, 0, 255, 1, 4, 0)),
            (Axis::ABS_MISC, AbsInfo::new(0, 0, 100, 0, 0, 0)),
        ];
        let axes = tehuti::controller::virtual_pad_axes(&source);
        let codes: Vec<_> = axes.iter().map(|(axis, _)| *axis).collect();
        assert_eq!(
            codes,
            [
                Axis::ABS_X,
                Axis::ABS_Y,
                Axis::ABS_Z,
                Axis::ABS_RX,
                Axis::ABS_RY,
                Axis::ABS_RZ,
                Axis::ABS_HAT0X,
                Axis::ABS_HAT0Y
            ]
        );
        let (_, x) = axes.iter().find(|(axis, _)| *axis == Axis::ABS_X).unwrap();
        assert_eq!((x.minimum(), x.maximum(), x.flat()), (0, 255, 4));
        let (_, hat) = axes.iter().find(|(axis, _)| *axis == Axis::ABS_HAT0X).unwrap();
        assert_eq!((hat.minimum(), hat.maximum()), (-1, 1));
    }

    #[test]
    fn every_advertised_axis_passes_kernel_absinfo_validation() {
        use evdev::{AbsInfo, AbsoluteAxisCode as Axis};
        let calibrations = [
            AbsInfo::new(0, 10, -10, 0, 0, 0),
            AbsInfo::new(0, -1, 1, 0, 3, 0),
            AbsInfo::new(0, 0, 0, 0, 1, 0),
            AbsInfo::new(0, i32::MIN, i32::MAX, 0, i32::MAX, 0),
            AbsInfo::new(128, 0, 255, 1, 4, 0),
        ];
        for info in calibrations {
            let source: Vec<_> = [
                Axis::ABS_X,
                Axis::ABS_Y,
                Axis::ABS_Z,
                Axis::ABS_RX,
                Axis::ABS_RY,
                Axis::ABS_RZ,
                Axis::ABS_HAT0X,
                Axis::ABS_HAT0Y,
            ]
            .into_iter()
            .map(|axis| (axis, info))
            .collect();
            for source in [&[][..], source.as_slice()] {
                for (axis, advertised) in tehuti::controller::virtual_pad_axes(source) {
                    assert!(advertised.maximum() >= advertised.minimum(), "{axis:?}");
                    if let Some(range) = advertised.maximum().checked_sub(advertised.minimum()) {
                        assert!(advertised.flat() <= range, "{axis:?}");
                    }
                    assert_ne!(axis, Axis::ABS_MT_SLOT);
                }
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_source_calibration_never_advertises_invalid_absinfo(
            value in proptest::prelude::any::<i32>(),
            minimum in proptest::prelude::any::<i32>(),
            maximum in proptest::prelude::any::<i32>(),
            fuzz in proptest::prelude::any::<i32>(),
            flat in proptest::prelude::any::<i32>(),
            resolution in proptest::prelude::any::<i32>(),
        ) {
            let axes = tehuti::controller::virtual_pad_axes(&[]);
            let source: Vec<_> = axes.iter().map(|(axis, _)| {
                (*axis, evdev::AbsInfo::new(value, minimum, maximum, fuzz, flat, resolution))
            }).collect();
            // https://github.com/torvalds/linux/blob/master/drivers/input/misc/uinput.c
            for (axis, info) in tehuti::controller::virtual_pad_axes(&source) {
                proptest::prop_assert!(info.maximum() >= info.minimum(), "{:?}", axis);
                if let Some(range) = info.maximum().checked_sub(info.minimum()) {
                    proptest::prop_assert!(info.flat() <= range, "{:?}", axis);
                }
                proptest::prop_assert_ne!(axis, evdev::AbsoluteAxisCode::ABS_MT_SLOT);
            }
        }
    }

    #[test]
    fn source_face_buttons_reach_the_same_xbox_positions() {
        use evdev::{EventType, InputEvent, InputId, KeyCode};
        for (vendor, product, north, west) in [
            (0x045e, 0x0b13, KeyCode::BTN_WEST, KeyCode::BTN_NORTH),
            (0x054c, 0x09cc, KeyCode::BTN_NORTH, KeyCode::BTN_WEST),
            (0x057e, 0x2009, KeyCode::BTN_NORTH, KeyCode::BTN_WEST),
        ] {
            let source = InputId::new(BusType::BUS_USB, vendor, product, 1);
            for value in [0, 1] {
                for (input, output) in [
                    (north, KeyCode::BTN_WEST),
                    (west, KeyCode::BTN_NORTH),
                    (KeyCode::BTN_SOUTH, KeyCode::BTN_SOUTH),
                    (KeyCode::BTN_EAST, KeyCode::BTN_EAST),
                ] {
                    let event = tehuti::controller::virtual_pad_event(
                        &source,
                        InputEvent::new(EventType::KEY.0, input.code(), value),
                    );
                    assert_eq!((event.code(), event.value()), (output.code(), value));
                }
            }
            let axis = InputEvent::new(EventType::ABSOLUTE.0, 0, 123);
            assert_eq!(tehuti::controller::virtual_pad_event(&source, axis), axis);
        }
    }
}
