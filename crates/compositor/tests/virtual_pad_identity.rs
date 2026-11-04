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
