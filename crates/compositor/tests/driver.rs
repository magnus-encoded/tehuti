#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        driver::{Driver, Request},
        state::Compositor,
    };

    #[test]
    fn nested_framebuffer_captures_have_the_same_orientation_as_headless_captures() {
        use smithay::utils::Transform;
        let directory = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let output = compositor.output.clone().unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let normal = directory.path().join("normal.png");
        let nested = directory.path().join("nested.png");
        let _normal_capture = driver
            .execute(&mut compositor, Request::Shot { path: normal.clone() })
            .unwrap();
        output.change_current_state(None, Some(Transform::Flipped180), None, None);
        let _nested_capture = driver
            .execute(&mut compositor, Request::Shot { path: nested.clone() })
            .unwrap();
        assert!(
            image::open(normal).unwrap().to_rgba8() == image::open(nested).unwrap().to_rgba8(),
            "nested captures retain the upright scene"
        );
    }

    #[test]
    fn live_sessions_cannot_create_a_driver() {
        let directory = tempfile::tempdir().unwrap();
        assert!(Driver::bind(directory.path(), tehuti::backend::Backend::Tty).is_err());
        assert!(!directory.path().join("tehuti-drive.sock").exists());
    }

    #[test]
    fn socket_is_private_and_owned_until_drop() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tehuti-drive.sock");
        let driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(Driver::bind(directory.path(), tehuti::backend::Backend::Nested).is_err());
        assert!(path.exists());
        drop(driver);
        assert!(!path.exists());
    }

    #[test]
    fn held_buttons_use_only_the_advanced_test_clock() {
        let directory = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        for button in [
            tehuti_protocol::ButtonName::Guide,
            tehuti_protocol::ButtonName::Start,
        ] {
            let _held = driver.execute(&mut compositor, Request::Hold { button }).unwrap();
        }
        let _before = driver
            .execute(&mut compositor, Request::Advance { milliseconds: 2999 })
            .unwrap();
        assert!(!compositor.quit_requested());
        let _released = driver
            .execute(
                &mut compositor,
                Request::Release {
                    button: tehuti_protocol::ButtonName::Start,
                },
            )
            .unwrap();
        let _after_release = driver
            .execute(&mut compositor, Request::Advance { milliseconds: 1 })
            .unwrap();
        assert!(!compositor.quit_requested());
        let _held_again = driver
            .execute(
                &mut compositor,
                Request::Hold {
                    button: tehuti_protocol::ButtonName::Start,
                },
            )
            .unwrap();
        let _after = driver
            .execute(&mut compositor, Request::Advance { milliseconds: 3000 })
            .unwrap();
        assert!(compositor.quit_requested());
    }

    #[test]
    fn driver_routes_speech_and_confirm_through_the_launcher() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("destinations.toml"),
            r#"
[[destinations]]
name = "Library"
[[destinations.routes]]
steps = [{ icon = "book", target = { kind = "action", value = { type = "notice", text = "Route reached" } } }]
"#,
        )
        .unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.configure_destinations(tehuti::destinations::Paths {
            config: directory.path().into(),
            projects: directory.path().join("projects"),
            data: vec![],
        });
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        assert_eq!(
            driver.execute(&mut compositor, Request::Scene).unwrap()["windows"],
            serde_json::json!([])
        );
        let _reply = driver
            .execute(
                &mut compositor,
                Request::Say {
                    text: "open library".into(),
                },
            )
            .unwrap();
        assert_eq!(
            driver.execute(&mut compositor, Request::Scene).unwrap()["launcher"]["rows"][0]["name"],
            "Library"
        );
        let _confirmation = driver
            .execute(
                &mut compositor,
                Request::Press {
                    button: tehuti_protocol::ButtonName::South,
                },
            )
            .unwrap();
        assert_eq!(compositor.visible_notice_text().as_deref(), Some("Route reached"));
        let _expired = driver
            .execute(&mut compositor, Request::Advance { milliseconds: 3000 })
            .unwrap();
        assert_eq!(compositor.visible_notice_text(), None);
    }
}
