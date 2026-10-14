//! Renderer policy tests need no DRM device or EGL context.

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use smithay::backend::allocator::{Format, Fourcc, Modifier, format::FormatSet};
    use tehuti::rendering::{Rendering, initialize, scanout_formats};

    #[test]
    fn gpu_success_never_initializes_pixman() {
        let software_called = Cell::new(false);
        let selected = initialize(
            || Ok::<_, &str>("gpu"),
            || {
                software_called.set(true);
                Ok::<_, &str>("software")
            },
        )
        .unwrap();
        assert!(matches!(selected, Rendering::Gles("gpu")), "GPU has priority");
        assert!(!software_called.get(), "pixman must stay lazy");
    }

    #[test]
    fn any_gpu_initialization_failure_selects_pixman() {
        for failure in ["GBM", "EGL", "context", "allocation", "scanout"] {
            let selected = initialize(|| Err::<(), _>(failure), || Ok::<_, &str>("software")).unwrap();
            assert!(
                matches!(selected, Rendering::Pixman("software")),
                "GPU failures fall back"
            );
        }
        let failed = initialize(|| Err::<(), _>("EGL"), || Err::<(), _>("pixman"));
        assert_eq!(
            failed.err(),
            Some("pixman"),
            "both failures return the software error"
        );
    }

    #[test]
    fn scanout_requires_matching_fourcc_and_modifier_and_prefers_opaque() {
        let argb = Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::Linear,
        };
        let xrgb = Format {
            code: Fourcc::Xrgb8888,
            modifier: Modifier::Linear,
        };
        let implicit = Format {
            code: Fourcc::Xrgb8888,
            modifier: Modifier::Invalid,
        };
        let texture_only = Format {
            code: Fourcc::Abgr8888,
            modifier: Modifier::Linear,
        };
        let renderer = FormatSet::from_iter([argb, xrgb, texture_only]);
        let plane = FormatSet::from_iter([implicit, argb, xrgb, texture_only]);
        assert_eq!(
            scanout_formats(&renderer, &plane),
            [xrgb, argb],
            "opaque RGB precedes ARGB and excludes other formats"
        );
        let incompatible = FormatSet::from_iter([implicit]);
        assert!(
            scanout_formats(&renderer, &incompatible).is_empty(),
            "matching fourcc alone is insufficient"
        );
        assert!(
            scanout_formats(&FormatSet::default(), &plane).is_empty(),
            "no rendering formats means no scanout"
        );
    }
}
