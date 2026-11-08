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
        for key in ["start_recording", "stop_recording"] {
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
                .map(|command| command.key()),
                Some(key)
            );
            assert!(menu.is_none(), "recording starts and stops on one press");
        }
    }

    #[test]
    fn voice_recording_controls_precede_plugins_and_launchers() {
        for (phrase, key) in [
            ("Start recording.", "start_recording"),
            ("STOP RECORDING!", "stop_recording"),
        ] {
            let mut state = State::default();
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
                    if let Effect::RunMenuCommand(command) = effect {
                        commands.push(command.key());
                    } else if let Effect::QueryPlugin(_) = effect {
                        panic!("recording phrases must precede plugins");
                    } else if let Effect::OpenLauncher(_) = effect {
                        panic!("recording phrases must precede the launcher");
                    } else if let Effect::ReadClock = effect {
                        input = Input::ClockRead(Duration::ZERO);
                    } else if let Effect::ConfirmSecret(_) = effect {
                        input = Input::SecretConfirmation(false);
                    } else if let Effect::PollSpeech(_) = effect {
                        input = Input::SpeechPolled(vec![]);
                    } else if let Effect::Supervise = effect {
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
