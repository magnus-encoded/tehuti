//! Renderer policy tests need no DRM device or EGL context.

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use smithay::backend::allocator::{Format, Fourcc, Modifier, format::FormatSet};
    use tehuti::rendering::{Rendering, initialize, scanout_formats};

    #[test]
    fn gpu_failure_after_startup_falls_back_once() {
        let mut rendering: Rendering<u8, u8> = Rendering::Gles(1);
        assert_eq!(rendering.fall_back(|| Ok::<_, ()>(2)), Ok(true));
        assert!(matches!(rendering, Rendering::Pixman(2)));
        assert_eq!(rendering.fall_back(|| Ok::<_, ()>(3)), Ok(false));
        assert!(matches!(rendering, Rendering::Pixman(2)));
    }

    fn run(
        rendering: &mut Rendering<u8, u8>,
        draw_fails: bool,
        submit_fails: bool,
    ) -> (Result<u8, String>, bool) {
        let mut switched = false;
        let drawn = tehuti::rendering::draw_with_fall_back(
            rendering,
            |rendering| match rendering {
                Rendering::Gles(_) if draw_fails => Err("draw".to_owned()),
                Rendering::Gles(value) | Rendering::Pixman(value) => Ok(*value),
            },
            || Ok::<_, String>(9),
            |_| switched = true,
        );
        // Submitting happens outside the seam, so its failure never reaches it.
        let result = drawn.and_then(|value| {
            if submit_fails {
                Err("submit".to_owned())
            } else {
                Ok(value)
            }
        });
        (result, switched)
    }

    #[test]
    fn a_draw_failure_switches_to_pixman_and_redraws() {
        let mut rendering = Rendering::Gles(1);
        assert_eq!(run(&mut rendering, true, false), (Ok(9), true));
        assert!(matches!(rendering, Rendering::Pixman(9)));
    }

    #[test]
    fn a_submit_failure_keeps_the_gpu_renderer() {
        let mut rendering = Rendering::Gles(1);
        assert_eq!(
            run(&mut rendering, false, true),
            (Err("submit".to_owned()), false)
        );
        assert!(matches!(rendering, Rendering::Gles(1)));
    }

    #[test]
    fn a_pixman_draw_failure_is_not_retried() {
        let mut rendering: Rendering<u8, u8> = Rendering::Pixman(1);
        let result = tehuti::rendering::draw_with_fall_back(
            &mut rendering,
            |_| Err::<u8, _>("draw".to_owned()),
            || Ok::<_, String>(9),
            |_| panic!("no switch from pixman"),
        );
        assert_eq!(result, Err("draw".to_owned()));
    }

    #[test]
    fn failed_fall_back_keeps_the_gpu_renderer() {
        let mut rendering: Rendering<u8, u8> = Rendering::Gles(1);
        assert_eq!(rendering.fall_back(|| Err::<u8, _>("no")), Err("no"));
        assert!(matches!(rendering, Rendering::Gles(1)));
    }

    #[test]
    fn mismatch_is_reported_only_when_no_format_matches() {
        let renderer = FormatSet::from_iter([Format {
            code: Fourcc::Xrgb8888,
            modifier: Modifier::Invalid,
        }]);
        let plane = FormatSet::from_iter([Format {
            code: Fourcc::Xrgb8888,
            modifier: Modifier::Linear,
        }]);
        assert!(tehuti::rendering::scanout_mismatch(&renderer, &plane).is_some());
        assert!(tehuti::rendering::scanout_mismatch(&renderer, &renderer).is_none());
    }

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
