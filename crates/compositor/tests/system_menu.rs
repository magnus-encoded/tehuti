#[cfg(test)]
mod system_menu {
    use tehuti::system_menu::Config;
    use tehuti_core::menu::Command;

    fn run(
        mut state: tehuti_core::dispatch::State,
        mut input: tehuti_core::dispatch::Input,
    ) -> (tehuti_core::dispatch::State, Vec<tehuti_core::dispatch::Effect>) {
        use tehuti_core::dispatch::{self, Effect, Input};
        let mut trace = Vec::new();
        loop {
            let (next, effects) = dispatch::step(state, input);
            state = next;
            if effects.is_empty() {
                return (state, trace);
            }
            input = effects
                .iter()
                .find_map(|effect| {
                    if matches!(effect, Effect::Supervise) {
                        Some(Input::Supervised(Vec::new()))
                    } else if let Effect::PollSpeech(_) = effect {
                        Some(Input::SpeechPolled(Vec::new()))
                    } else if let Effect::ConfirmSecret(_) = effect {
                        Some(Input::SecretConfirmation(false))
                    } else if let Effect::QueryPlugin(_) = effect {
                        Some(Input::PluginActive(false))
                    } else if let Effect::DispatchPlugin { .. } = effect {
                        Some(Input::PluginRequest(None))
                    } else {
                        None
                    }
                })
                .unwrap_or(Input::Resume);
            trace.extend(effects);
        }
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

    const CUSTOM_ENTRIES: &str = r#"
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
"#;

    #[test]
    fn custom_entries_follow_builtins_in_order_and_spawn_their_argv() {
        use tehuti_core::{
            State,
            dispatch::{self, Input},
        };
        use tehuti_protocol::{ButtonName, ButtonState, Event};
        let config = Config::parse(CUSTOM_ENTRIES).unwrap();
        let mut core = State::new();
        core.set_menu_entries(config.labels());
        let mut state = dispatch::State::new(core);
        let button = |name, pressed| Event::Button { name, state: pressed };
        let (opened, _) = run(
            state,
            Input::Event {
                event: button(ButtonName::Guide, ButtonState::Pressed),
                elapsed: core::time::Duration::ZERO,
            },
        );
        state = opened;
        let (mut state, _) = run(
            state,
            Input::Tick {
                elapsed: core::time::Duration::from_millis(500),
            },
        );
        assert!(
            state
                .core()
                .menu()
                .unwrap()
                .text()
                .contains("SETTINGS\n  Terminal\n  Peer\n  Later")
        );
        for name in [ButtonName::DpadDown; 7].into_iter().chain([ButtonName::South]) {
            for pressed in [ButtonState::Pressed, ButtonState::Released] {
                let (next, effects) = run(
                    state,
                    Input::Event {
                        event: button(name, pressed),
                        elapsed: core::time::Duration::ZERO,
                    },
                );
                state = next;
                for effect in effects {
                    if let dispatch::Effect::RunMenuCommand(command) = effect {
                        assert_eq!(command, Command::Custom(0));
                        config
                            .run(command, |program, args| {
                                assert_eq!(program, "terminal");
                                assert_eq!(args, ["--title", "two words"]);
                                Ok(())
                            })
                            .unwrap();
                        return;
                    }
                }
            }
        }
        panic!("custom command was not selected");
    }

    #[test]
    fn invalid_entries_are_reported_without_disabling_valid_commands() {
        let config = Config::parse("unknown = [1]\nreboot = [\"session-reboot\"]").unwrap();
        assert!(config.notices()[0].contains("unknown"));
        config
            .run(Command::Reboot, |program, args| {
                assert_eq!(program, "session-reboot");
                assert_eq!(args, Vec::<String>::new());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn menu_command_receives_the_compositor_socket() {
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
exec = ["/bin/sh", "-c", "printf '%s' \"$WAYLAND_DISPLAY\" > \"$TEHUTI_MENU_CAPTURE\""]"#
            } else {
                r#"task_monitor = ["/bin/sh", "-c", "printf '%s' \"$WAYLAND_DISPLAY\" > \"$TEHUTI_MENU_CAPTURE\""]"#
            },
        )
        .unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "system_menu::isolated_menu_probe"])
            .env("TEHUTI_MENU_CONFIG", config)
            .env("TEHUTI_MENU_CAPTURE", &capture)
            .env("TEHUTI_TEST_CUSTOM_MENU", if custom { "yes" } else { "no" })
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(
            std::fs::read_to_string(capture).unwrap(),
            "example-compositor-socket"
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
