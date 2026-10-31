#[cfg(test)]
mod system_menu {
    use tehuti::system_menu::Config;
    use tehuti_core::menu::Command;

    #[test]
    fn defaults_work_without_programs_or_a_menu_file() {
        let directory = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "system_menu::isolated_default_menu_probe"])
            .env("TEHUTI_TEST_DEFAULT_MENU", "yes")
            .env("TEHUTI_MENU_CONFIG", directory.path().join("missing.toml"))
            .env("PATH", "")
            .env("XDG_DATA_HOME", directory.path())
            .env("XDG_DATA_DIRS", directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn isolated_default_menu_probe() {
        use core::time::Duration;
        use smithay::reexports::wayland_server::Display;
        use tehuti::state::Compositor;
        use tehuti_protocol::{ButtonName, ButtonState, Event};
        if std::env::var_os("TEHUTI_TEST_DEFAULT_MENU").is_none() {
            return;
        }
        for index in [0, 2, 5] {
            let display: Display<Compositor> = Display::new().unwrap();
            let mut compositor = Compositor::new(&display.handle()).unwrap();
            compositor.set_launch_socket("example-menu-socket".into());
            let press = |target: &mut Compositor, name| {
                for state in [ButtonState::Pressed, ButtonState::Released] {
                    target.handle(&Event::Button { name, state });
                }
            };
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
            for _ in 0..index {
                press(&mut compositor, ButtonName::DpadDown);
            }
            press(&mut compositor, ButtonName::South);
            assert!(!compositor.quit_requested());
            if index == 5 {
                assert!(
                    compositor
                        .visible_notice_text()
                        .unwrap()
                        .contains("plasma-systemmonitor")
                );
                continue;
            }
            press(&mut compositor, ButtonName::South);
            assert!(compositor.quit_requested());
            assert_eq!(compositor.log_out_requested(), index == 0);
            assert!(!compositor.restart_requested());
        }
    }

    #[test]
    fn session_defaults_match_the_launcher_and_respect_overrides() {
        let defaults = Config::default();
        assert_eq!(defaults.session_exit_status(Command::LogOut), Some(21));
        assert_eq!(defaults.session_exit_status(Command::Plasma), Some(20));
        let config = Config::parse("log_out = []\nplasma = [\"example-session\"]").unwrap();
        assert_eq!(config.session_exit_status(Command::LogOut), None);
        assert_eq!(config.session_exit_status(Command::Plasma), None);
        assert!(
            config
                .run(Command::LogOut, |_, _| panic!("disabled logout ran"))
                .is_err()
        );
        config
            .run(Command::Plasma, |program, _| {
                assert_eq!(program, "example-session");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn task_monitor_uses_the_default_program_and_preserves_spawn_errors() {
        Config::default()
            .run(Command::TaskMonitor, |program, args| {
                assert_eq!(program, "plasma-systemmonitor");
                assert_eq!(args, Vec::<String>::new());
                Ok(())
            })
            .unwrap();
        let config = Config::parse("task_monitor = [\"example-monitor\"]").unwrap();
        let error = config
            .run(Command::TaskMonitor, |_, _| {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            })
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("example-monitor"));
    }

    proptest::proptest! {
        #[test]
        fn broken_custom_entries_preserve_valid_neighbors(
            broken in proptest::sample::select(vec![
                "label = 1\nexec = [\"tool\"]\norder = 0",
                "label = \"\"\nexec = [\"tool\"]\norder = 0",
                "label = \"Bad\"\nexec = []\norder = 0",
                "label = \"Bad\"\nexec = [1]\norder = 0",
                "label = \"Bad\"\nexec = [\"tool\"]\norder = \"first\"",
                "label = \"Bad\"\nexec = [\"tool\"]",
                "label = \"Bad\"\nexec = [\"tool\"]\norder = 0\nextra = true",
                "label = \"Bad\\nLabel\"\nexec = [\"tool\"]\norder = 0",
                "label = \"Bad\"\nexec = [\"bad\\u0000name\"]\norder = 0",
            ])
        ) {
            let text = format!("[[entry]]\n{broken}\n[[entry]]\nlabel = \"Valid\"\nexec = [\"tool\"]\norder = 1");
            let config = Config::parse(&text).unwrap();
            proptest::prop_assert_eq!(config.notices().len(), 1);
            proptest::prop_assert!(config.notices()[0].contains("entry[0]"));
            proptest::prop_assert_eq!(config.labels(), vec!["Valid"]);
            let result = config.run(Command::Custom(0), |program, _| {
                assert_eq!(program, "tool");
                Ok(())
            });
            proptest::prop_assert!(result.is_ok());
        }
    }

    #[test]
    fn user_entries_follow_builtins_in_order_and_run_their_argv() {
        use tehuti_core::{
            menu::{self, Menu},
            pad::PadFamily,
        };
        use tehuti_protocol::{ButtonName, ButtonState, Event};
        let config = Config::parse(
            r#"
[[entry]]
label = "Later"
exec = ["later"]
order = 20
[[entry]]
label = "Terminal"
exec = ["terminal", "--title", "two words"]
order = 10
[[entry]]
label = "Peer"
exec = ["peer"]
order = 10
"#,
        )
        .unwrap();
        assert_eq!(config.labels(), ["Terminal", "Peer", "Later"]);
        let mut menu = Some(Menu::new(PadFamily::default()).with_entries(config.labels()));
        assert!(
            menu.as_ref()
                .unwrap()
                .text()
                .contains("SETTINGS\n  Terminal\n  Peer\n  Later")
        );
        for _ in 0..7 {
            assert_eq!(
                menu::handle(
                    &mut menu,
                    &Event::Button {
                        name: ButtonName::DpadDown,
                        state: ButtonState::Pressed
                    }
                ),
                None
            );
        }
        let command = menu::handle(
            &mut menu,
            &Event::Button {
                name: ButtonName::South,
                state: ButtonState::Pressed,
            },
        )
        .unwrap();
        assert_eq!(command, Command::Custom(0));
        assert!(menu.is_none());
        config
            .run(command, |program, args| {
                assert_eq!(program, "terminal");
                assert_eq!(args, ["--title", "two words"]);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn invalid_entries_preserve_valid_commands() {
        let config = Config::parse("unknown = [1]\nreboot = [\"session-reboot\"]").unwrap();
        config
            .run(Command::Reboot, |program, args| {
                assert_eq!(program, "session-reboot");
                assert_eq!(args, Vec::<String>::new());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn menu_command_receives_the_compositor_socket_and_session_display() {
        check_socket(false);
        check_socket(true);
    }

    fn check_socket(custom: bool) {
        let directory = tempfile::tempdir().unwrap();
        let capture = directory.path().join("capture");
        let config = directory.path().join("menu.toml");
        std::fs::write(
            &config,
            if custom {
                r#"unknown = ["ignored"]
[[entry]]
label = "Capture"
order = 0
exec = ["/bin/sh", "-c", "printf '%s %s' \"$WAYLAND_DISPLAY\" \"$DISPLAY\" > \"$TEHUTI_MENU_CAPTURE\""]"#
            } else {
                r#"task_monitor = ["/bin/sh", "-c", "printf '%s %s' \"$WAYLAND_DISPLAY\" \"$DISPLAY\" > \"$TEHUTI_MENU_CAPTURE\""]"#
            },
        )
        .unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "system_menu::isolated_menu_probe"])
            .env("TEHUTI_MENU_CONFIG", config)
            .env("TEHUTI_MENU_CAPTURE", &capture)
            .env("TEHUTI_TEST_CUSTOM_MENU", if custom { "yes" } else { "no" })
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
        if std::env::var("TEHUTI_TEST_CUSTOM_MENU").as_deref() == Ok("yes") {
            assert!(compositor.visible_notice_text().unwrap().contains("unknown"));
            compositor.set_contexts(Vec::new());
            std::fs::write(std::env::var_os("TEHUTI_MENU_CONFIG").unwrap(), "invalid TOML").unwrap();
        }
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
        let count = if std::env::var("TEHUTI_TEST_CUSTOM_MENU").as_deref() == Ok("yes") {
            7
        } else {
            5
        };
        for name in core::iter::repeat_n(ButtonName::DpadDown, count).chain([ButtonName::South]) {
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
            r#"unknown = ["command"]"#,
            r#"reboot = [""]"#,
            "reboot = [1]",
            r#"reboot = ["bad\u0000name"]"#,
        ] {
            assert_ne!(Config::parse(text).unwrap().notices(), Vec::<String>::new());
        }
        assert!(Config::parse("not TOML [").is_err());
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
