#[cfg(test)]
mod recording {
    use core::time::Duration;
    use tehuti_core::{
        dispatch::{Effect, Input, State, step},
        menu::{Menu, handle},
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    #[test]
    fn recording_controls_are_available_without_session_confirmation() {
        let key = "toggle_recording";
        let item = tehuti_core::menu::ITEMS
            .iter()
            .find(|command| command.key() == key);
        assert!(
            item.is_some(),
            "recording control must be offered in the system menu"
        );
        let mut menu = Some(Menu::for_command(
            tehuti_core::pad::PadFamily::Letters,
            *item.unwrap(),
        ));
        assert_eq!(
            handle(
                &mut menu,
                &Event::Button {
                    name: ButtonName::South,
                    state: ButtonState::Pressed
                }
            )
            .map(tehuti_core::menu::Command::key),
            Some(key)
        );
        assert!(menu.is_none(), "recording starts and stops on one press");
    }

    #[test]
    fn recording_menu_label_shows_the_available_action() {
        let menu = Menu::new(tehuti_core::pad::PadFamily::Letters);
        let idle = menu.text();
        assert_eq!(idle.matches("START RECORDING").count(), 1);
        assert!(!idle.contains("STOP RECORDING"));
        let active = menu.with_recording(true).text();
        assert_eq!(active.matches("STOP RECORDING").count(), 1);
        assert!(!active.contains("START RECORDING"));
    }

    #[test]
    fn voice_recording_controls_precede_plugins_and_launchers() {
        for (phrase, key) in [
            ("Start recording.", "start_recording"),
            ("STOP RECORDING!", "stop_recording"),
        ] {
            let mut state = step(
                State::default(),
                Input::Launcher(tehuti_core::launcher::Launcher::invitation(None)),
            )
            .0;
            let mut input = Input::Event {
                event: Event::Speech {
                    text: Text::new(phrase.into()).unwrap(),
                },
                elapsed: Duration::ZERO,
            };
            let mut commands = Vec::new();
            for _ in 0..32 {
                let (next, effects) = step(state, input);
                state = next;
                input = Input::Resume;
                for effect in effects {
                    assert!(
                        !matches!(effect, Effect::QueryPlugin(Event::Speech { .. })),
                        "recording phrases must precede plugins"
                    );
                    assert!(
                        !matches!(effect, Effect::OpenLauncher(_)),
                        "recording phrases must precede the launcher"
                    );
                    if let Effect::RunMenuCommand(command) = effect {
                        commands.push(command.key());
                    } else if let Effect::QueryPlugin(_) = effect {
                        input = Input::PluginActive(true);
                    } else if let Effect::DispatchPlugin { .. } = effect {
                        input = Input::PluginRequest(None);
                    } else if effect == Effect::ReadClock {
                        input = Input::ClockRead(Duration::ZERO);
                    } else if let Effect::ConfirmSecret(_) = effect {
                        input = Input::SecretConfirmation(false);
                    } else if let Effect::PollSpeech(_) = effect {
                        input = Input::SpeechPolled(vec![]);
                    } else if effect == Effect::Supervise {
                        input = Input::Supervised(vec![]);
                    }
                }
                if state.busy() == 0 {
                    break;
                }
            }
            assert_eq!(commands, vec![key]);
        }
    }

    #[test]
    fn recording_indicator_preserves_plugin_status() {
        let bar = tehuti_core::bar::Bar::new()
            .with_status("Build passing".into())
            .with_recording(true);
        let texts: Vec<_> = bar.segments().into_iter().map(|segment| segment.text).collect();
        assert!(texts.contains(&"REC".into()));
        assert!(texts.contains(&"Build passing".into()));
        assert!(
            !bar.with_recording(false)
                .segments()
                .iter()
                .any(|segment| segment.text == "REC")
        );
    }
}
