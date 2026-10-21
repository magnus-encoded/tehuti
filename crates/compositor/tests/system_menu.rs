#[cfg(test)]
mod system_menu {
    use tehuti::system_menu::Config;
    use tehuti_core::menu::Command;

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
}
