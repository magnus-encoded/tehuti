#[cfg(test)]
mod system_menu {
    use tehuti::system_menu::Config;
    use tehuti_core::menu::Command;

    #[test]
    fn menu_command_receives_the_compositor_socket_and_session_display() {
        let directory = tempfile::tempdir().unwrap();
        let capture = directory.path().join("capture");
        let config = directory.path().join("menu.toml");
        std::fs::write(
            &config,
            r#"task_monitor = ["/bin/sh", "-c", "printf '%s %s' \"$WAYLAND_DISPLAY\" \"$DISPLAY\" > \"$TEHUTI_MENU_CAPTURE\""]"#,
        )
        .unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "system_menu::isolated_menu_probe"])
            .env("TEHUTI_MENU_CONFIG", config)
            .env("TEHUTI_MENU_CAPTURE", &capture)
            .env_remove("WAYLAND_DISPLAY")
            .env("DISPLAY", ":99")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(
            std::fs::read_to_string(capture).unwrap(),
            "example-compositor-socket :42"
        );
    }

    #[test]
    fn isolated_menu_probe() {
        use core::time::Duration;
        use smithay::reexports::wayland_server::Display;
        use tehuti::state::Compositor;
        use tehuti_protocol::{ButtonName, ButtonState, Event};

        let Some(capture) = std::env::var_os("TEHUTI_MENU_CAPTURE") else {
            return;
        };

        tehuti::child_process::configure_display(Some(":42".into())).unwrap();
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_launch_socket("example-compositor-socket".into());
        compositor.handle(&Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Pressed,
        });
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(500),
        });
        compositor.handle(&Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Released,
        });
        for name in [ButtonName::DpadDown; 5].into_iter().chain([ButtonName::South]) {
            compositor.handle(&Event::Button {
                name,
                state: ButtonState::Pressed,
            });
            compositor.handle(&Event::Button {
                name,
                state: ButtonState::Released,
            });
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if std::fs::read_to_string(&capture).is_ok_and(|text| !text.is_empty()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn commands_use_only_configured_arguments() {
        let config = Config::parse(r#"reboot = ["example-session", "restart", "an argument"]"#).unwrap();
        config
            .run(Command::Reboot, |program, args| {
                assert_eq!(program, "example-session");
                assert_eq!(args, ["restart", "an argument"]);
                Ok(())
            })
            .unwrap();
        assert!(
            config
                .run(Command::LogOut, |_, _| panic!("unconfigured command ran"))
                .is_err()
        );
    }

    #[test]
    fn invalid_config_and_empty_commands_cannot_run() {
        for text in [
            r#"{"reboot":["command"]}"#,
            r#"unknown = ["command"]"#,
            r#"reboot = [""]"#,
            "reboot = [1]",
            r#"reboot = ["bad\u0000name"]"#,
        ] {
            assert!(Config::parse(text).is_err());
        }
        let config = Config::parse("shut_down = []").unwrap();
        assert!(
            config
                .run(Command::ShutDown, |_, _| panic!("disabled command ran"))
                .is_err()
        );
    }

    #[test]
    fn example_configures_every_menu_command() {
        let config = Config::parse(include_str!("../../../examples/menu.toml")).unwrap();
        for command in [
            Command::LogOut,
            Command::Plasma,
            Command::Reboot,
            Command::ShutDown,
            Command::TaskMonitor,
        ] {
            config.run(command, |_, _| Ok(())).unwrap();
        }
    }

    #[test]
    fn menu_overlay_closes_and_escape_needs_no_renderer() {
        use core::time::Duration;
        use smithay::reexports::wayland_server::Display;
        use tehuti::state::Compositor;
        use tehuti_protocol::{ButtonName, ButtonState, Event};

        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.handle(&Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Pressed,
        });
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(500),
        });
        assert!(
            compositor
                .visible_notice_text()
                .is_some_and(|text| text.contains("SYSTEM MENU") && text.contains("SWITCH TO PLASMA"))
        );
        compositor.handle(&Event::Button {
            name: ButtonName::East,
            state: ButtonState::Pressed,
        });
        assert!(compositor.visible_notice_text().is_none());
        compositor.handle(&Event::Button {
            name: ButtonName::Start,
            state: ButtonState::Pressed,
        });
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_secs(3),
        });
        assert!(compositor.quit_requested());
    }

    #[test]
    fn confirmed_restart_stops_the_compositor_without_a_menu_config() {
        use core::time::Duration;
        use smithay::reexports::wayland_server::Display;
        use tehuti::state::Compositor;
        use tehuti_core::controller::Input;
        use tehuti_protocol::{ButtonName, ButtonState, Event};

        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let pad = |target: &mut Compositor, name, state| {
            target.controller_input(Input::Event(Event::Button { name, state }));
        };
        assert!(!compositor.restart_requested());
        pad(&mut compositor, ButtonName::Guide, ButtonState::Pressed);
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(500),
        });
        pad(&mut compositor, ButtonName::Guide, ButtonState::Released);
        pad(&mut compositor, ButtonName::DpadDown, ButtonState::Pressed);
        pad(&mut compositor, ButtonName::DpadDown, ButtonState::Released);
        pad(&mut compositor, ButtonName::South, ButtonState::Pressed);
        assert!(!compositor.quit_requested());
        assert!(!compositor.restart_requested());
        pad(&mut compositor, ButtonName::South, ButtonState::Released);
        pad(&mut compositor, ButtonName::South, ButtonState::Pressed);
        assert!(compositor.quit_requested());
        assert!(compositor.restart_requested());
        assert!(compositor.visible_notice_text().is_none());
    }

    #[test]
    fn settings_entry_opens_the_view_in_place_of_the_on_screen_keyboard() {
        use core::time::Duration;
        use smithay::reexports::wayland_server::Display;
        use tehuti::state::Compositor;
        use tehuti_core::controller::Input;
        use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.handle(&Event::Speech {
            text: Text::new("keyboard".into()).unwrap(),
        });
        assert!(compositor.on_screen_keyboard().is_open());
        let pad = |target: &mut Compositor, name, state| {
            target.controller_input(Input::Event(Event::Button { name, state }));
        };
        pad(&mut compositor, ButtonName::Guide, ButtonState::Pressed);
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(500),
        });
        pad(&mut compositor, ButtonName::Guide, ButtonState::Released);
        for name in [ButtonName::DpadDown; 6].into_iter().chain([ButtonName::South]) {
            pad(&mut compositor, name, ButtonState::Pressed);
            pad(&mut compositor, name, ButtonState::Released);
        }
        assert!(compositor.settings().open);
        assert!(!compositor.on_screen_keyboard().is_open());
        assert!(compositor.visible_notice_text().is_none());
        assert!(compositor.settings_buffer((1024, 600)).is_some());
    }
}
