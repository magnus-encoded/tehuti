#[cfg(test)]
mod scale {
    use proptest::prelude::*;
    use smithay::{
        output::{Mode, Output, PhysicalProperties, Subpixel},
        reexports::wayland_server::Display,
    };
    use tehuti::{scale, state::Compositor};

    #[test]
    fn default_scale_doubles_only_wide_modes() {
        assert_eq!(scale::default_scale(3840), 2);
        assert_eq!(scale::default_scale(2561), 2);
        assert_eq!(scale::default_scale(2560), 1);
        assert_eq!(scale::default_scale(1920), 1);
    }

    #[test]
    fn valid_values_override_the_default() {
        assert_eq!(scale::choose(Some("1".as_ref()), 3840), Ok(1));
        assert_eq!(scale::choose(Some("3".as_ref()), 1920), Ok(3));
        assert_eq!(scale::choose(None, 3840), Ok(2));
    }

    #[test]
    fn invalid_values_fall_back_with_a_warning() {
        for (value, width, fallback) in [
            ("0", 3840, 2),
            ("-1", 1920, 1),
            ("two", 3840, 2),
            ("1.5", 1920, 1),
            ("", 3840, 2),
        ] {
            let (scale, warning) = scale::choose(Some(value.as_ref()), width).unwrap_err();
            assert_eq!(scale, fallback, "{value}");
            assert!(warning.contains("TEHUTI_SCALE"), "{warning}");
        }
    }

    #[test]
    fn non_unicode_values_fall_back_with_a_warning() {
        use std::os::unix::ffi::OsStrExt;
        let value = std::ffi::OsStr::from_bytes(b"2\xff");
        let (scale, warning) = scale::choose(Some(value), 3840).unwrap_err();
        assert_eq!(scale, 2);
        assert!(warning.contains("TEHUTI_SCALE"), "{warning}");
    }

    proptest! {
        #[test]
        fn logical_size_divides_the_mode_by_the_scale(
            width in 1..8000_i32,
            height in 1..8000_i32,
            factor in 1..5_i32,
        ) {
            let logical = scale::logical_size((width.checked_mul(factor).unwrap(), height.checked_mul(factor).unwrap()).into(), factor);
            prop_assert_eq!((logical.w, logical.h), (width, height));
        }
    }

    #[test]
    fn layout_uses_the_logical_output_size_at_scale_two() {
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        let mode = Mode {
            size: (3840, 2160).into(),
            refresh: 60_000,
        };
        scale::apply(&output, mode, scale::default_scale(mode.size.w));
        assert_eq!(output.current_scale().integer_scale(), 2);
        compositor.space.map_output(&output, (0, 0));
        compositor.output = Some(output);
        compositor.fill_output();
        let geometry = compositor.layout_snapshot().outputs[0].geometry;
        let area = geometry.area;
        assert_eq!(
            geometry.usable.height, 1048.0,
            "the bar reserves 32 logical pixels"
        );
        assert_eq!(
            (area.x, area.y, area.width, area.height),
            (0.0, 0.0, 1920.0, 1080.0)
        );
    }
}
