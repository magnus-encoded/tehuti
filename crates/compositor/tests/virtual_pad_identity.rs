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
    fn virtual_pad_name_and_phys_are_fixed() {
        assert_eq!(tehuti::controller::VIRTUAL_PAD_NAME, "Tehuti virtual pad");
        assert_eq!(tehuti::controller::VIRTUAL_PAD_PHYS, c"tehuti/virtual-pad");
    }

    #[test]
    fn virtual_pad_advertises_the_xbox_axis_ranges() {
        use evdev::AbsoluteAxisCode as Axis;
        let advertised: Vec<_> = tehuti::controller::virtual_pad_axes()
            .into_iter()
            .map(|(axis, info)| (axis, info.minimum(), info.maximum(), info.flat()))
            .collect();
        assert_eq!(
            advertised,
            [
                (Axis::ABS_X, -32768, 32767, 0),
                (Axis::ABS_Y, -32768, 32767, 0),
                (Axis::ABS_Z, 0, 255, 0),
                (Axis::ABS_RX, -32768, 32767, 0),
                (Axis::ABS_RY, -32768, 32767, 0),
                (Axis::ABS_RZ, 0, 255, 0),
                (Axis::ABS_HAT0X, -1, 1, 0),
                (Axis::ABS_HAT0Y, -1, 1, 0),
            ]
        );
    }

    #[test]
    fn a_byte_stick_reaches_the_full_xbox_range() {
        use evdev::{AbsInfo, AbsoluteAxisCode as Axis, EventType, InputEvent};
        let source = [(Axis::ABS_X, AbsInfo::new(128, 0, 255, 1, 4, 0))];
        let shown = |value| {
            tehuti::controller::virtual_pad_value(
                &source,
                InputEvent::new(EventType::ABSOLUTE.0, Axis::ABS_X.0, value),
            )
            .value()
        };
        assert_eq!((shown(0), shown(127), shown(255)), (-32768, 0, 32767));
        let hat = InputEvent::new(EventType::ABSOLUTE.0, Axis::ABS_HAT0X.0, -1);
        assert_eq!(tehuti::controller::virtual_pad_value(&source, hat), hat);
    }

    proptest::proptest! {
        #[test]
        fn scaling_maps_ends_and_centre_exactly(
            minimum in proptest::prelude::any::<i32>(),
            maximum in proptest::prelude::any::<i32>(),
            target in proptest::sample::select(vec![(-32768, 32767), (0, 255)]),
        ) {
            proptest::prop_assume!(maximum > minimum);
            let scale = |value| tehuti::controller::scale_axis(value, (minimum, maximum), target);
            proptest::prop_assert_eq!(scale(minimum), target.0);
            proptest::prop_assert_eq!(scale(maximum), target.1);
            proptest::prop_assert_eq!(scale(minimum.midpoint(maximum)), target.0.midpoint(target.1));
        }

        #[test]
        fn scaling_is_monotonic_and_stays_in_range(
            minimum in proptest::prelude::any::<i32>(),
            maximum in proptest::prelude::any::<i32>(),
            first in proptest::prelude::any::<i32>(),
            second in proptest::prelude::any::<i32>(),
            target in proptest::sample::select(vec![(-32768, 32767), (0, 255)]),
        ) {
            proptest::prop_assume!(maximum > minimum);
            let (low, high) = (first.min(second), first.max(second));
            let scale = |value| tehuti::controller::scale_axis(value, (minimum, maximum), target);
            proptest::prop_assert!(scale(low) <= scale(high));
            proptest::prop_assert!((target.0..=target.1).contains(&scale(low)));
            proptest::prop_assert!((target.0..=target.1).contains(&scale(high)));
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
