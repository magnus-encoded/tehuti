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

    fn drive(state: State, input: Input) -> (State, Vec<Effect>) {
        drive_with_plugin(state, input, true)
    }

    fn drive_with_plugin(mut state: State, mut input: Input, plugin_active: bool) -> (State, Vec<Effect>) {
        let mut collected = Vec::new();
        for _ in 0..32 {
            let (next, effects) = step(state, input);
            state = next;
            input = Input::Resume;
            for effect in effects {
                if let Effect::QueryPlugin(_) = effect {
                    input = Input::PluginActive(plugin_active);
                } else if let Effect::DispatchPlugin { .. } = effect {
                    input = Input::PluginRequest(None);
                } else if effect == Effect::ReadClock {
                    input = Input::ClockRead(Duration::ZERO);
                } else if let Effect::ConfirmSecret(_) = effect {
                    input = Input::SecretConfirmation(false);
                } else if let Effect::SpeechCommand(_) = effect {
                    input = Input::SpeechSent(true);
                } else if let Effect::PollSpeech(_) = effect {
                    input = Input::SpeechPolled(vec![]);
                } else if effect == Effect::Supervise {
                    input = Input::Supervised(vec![]);
                }
                collected.push(effect);
            }
            if state.busy() == 0 {
                break;
            }
        }
        (state, collected)
    }

    fn press_button(state: State, name: ButtonName, button: ButtonState) -> (State, Vec<Effect>) {
        drive(
            state,
            Input::Event {
                event: Event::Button { name, state: button },
                elapsed: Duration::ZERO,
            },
        )
    }

    fn tap_button(state: State, name: ButtonName) -> (State, Vec<Effect>) {
        let (state, mut effects) = press_button(state, name, ButtonState::Pressed);
        let (state, released) = press_button(state, name, ButtonState::Released);
        effects.extend(released);
        (state, effects)
    }

    fn stick_step(state: State, vertical: i16) -> State {
        use tehuti_core::dispatch::Sticks;
        let (state, _) = drive(
            state,
            Input::Sticks(Sticks {
                left: (0, vertical),
                right: (0, 0),
            }),
        );
        drive(state, Input::Sticks(Sticks::default())).0
    }

    fn open_capture_chord(state: State) -> State {
        let (state, _) = press_button(state, ButtonName::Guide, ButtonState::Pressed);
        let (state, _) = press_button(state, ButtonName::RightBumper, ButtonState::Pressed);
        let (state, _) = press_button(state, ButtonName::RightBumper, ButtonState::Released);
        let (state, _) = press_button(state, ButtonName::Guide, ButtonState::Released);
        state
    }

    fn choice_names(state: &State) -> Vec<String> {
        state
            .choice_list()
            .unwrap()
            .candidates()
            .iter()
            .map(|row| row.candidate.name.clone())
            .collect()
    }

    #[test]
    fn voice_recording_controls_precede_plugins_and_launchers() {
        let shadowing = tehuti_protocol::parse_context(
            "app_id = 'example-app'\n[buttons]\n[speech]\nscreenshot = { notify = 'wrong' }\n'screenshot window' = { notify = 'wrong' }\n'start recording' = { notify = 'wrong' }\n'stop recording' = { notify = 'wrong' }",
        )
        .unwrap();
        for (phrase, key) in [
            ("Screenshot.", "screenshot"),
            ("screenshot window", "screenshot_window"),
            ("SCREENSHOT WINDOW!", "screenshot_window"),
            ("Start recording.", "start_recording"),
            ("STOP RECORDING!", "stop_recording"),
        ] {
            let state = State::new(tehuti_core::State::with_contexts(vec![shadowing.clone()]));
            let (state, _) = drive(
                state,
                Input::Event {
                    event: Event::Focus {
                        app_id: Some("example-app".into()),
                        title: String::new(),
                    },
                    elapsed: Duration::ZERO,
                },
            );
            let (state, _) = step(
                state,
                Input::Launcher(Box::new(tehuti_core::launcher::Launcher::invitation(None))),
            );
            let (_, effects) = drive(
                state,
                Input::Event {
                    event: Event::Speech {
                        text: Text::new(phrase.into()).unwrap(),
                    },
                    elapsed: Duration::ZERO,
                },
            );
            let mut commands = Vec::new();
            for effect in effects {
                assert!(
                    !matches!(effect, Effect::QueryPlugin(Event::Speech { .. })),
                    "capture phrases must precede plugins"
                );
                assert!(
                    !matches!(effect, Effect::OpenLauncher(_)),
                    "capture phrases must precede the launcher"
                );
                assert!(
                    !matches!(effect, Effect::Apply(_)),
                    "contexts must not shadow capture phrases"
                );
                if let Effect::RunMenuCommand(command) = effect {
                    commands.push(command.key());
                }
            }
            assert_eq!(commands, vec![key]);
        }
    }

    #[test]
    fn every_capture_unit_shows_the_exact_phrase_voice_uses() {
        for recording in [false, true] {
            let options = tehuti_core::menu::capture_options(recording);
            let slots = tehuti_core::menu::capture_choice_slots(recording);
            assert_eq!(slots.candidates.len(), options.len());
            for ((command, label, _), candidate) in options.into_iter().zip(&slots.candidates) {
                assert_eq!(candidate.name, label);
                assert_eq!(
                    tehuti_core::menu::capture_phrase(&candidate.name),
                    Some(command),
                    "capture choice unit must match voice phrase"
                );
            }
        }
        for (recording, expected) in [
            (false, tehuti_core::menu::Command::StartRecording),
            (true, tehuti_core::menu::Command::StopRecording),
        ] {
            let menu = Menu::new(tehuti_core::pad::PadFamily::Letters).with_recording(recording);
            let text = menu.text();
            assert!(text.contains("SCREENSHOT"), "system menu must show SCREENSHOT");
            assert_eq!(
                tehuti_core::menu::capture_phrase("SCREENSHOT"),
                Some(tehuti_core::menu::Command::Screenshot)
            );
            let record_label = if recording {
                "STOP RECORDING"
            } else {
                "START RECORDING"
            };
            assert!(
                text.contains(record_label),
                "system menu must show {record_label}"
            );
            assert_eq!(tehuti_core::menu::capture_phrase(record_label), Some(expected));
        }
    }

    #[test]
    fn pad_chord_opens_capture_choice_steps_with_stick_and_confirms_or_cancels() {
        use tehuti_core::choice_list::Choice;
        use tehuti_core::menu::Command;

        for family in [
            tehuti_core::pad::PadFamily::Letters,
            tehuti_core::pad::PadFamily::Nintendo,
        ] {
            let confirm = family.button(tehuti_core::pad::Role::Confirm);
            let back = family.button(tehuti_core::pad::Role::Back);
            let mut core = tehuti_core::State::default();
            core.set_pad_family(family);
            let level = core.level();
            let state = open_capture_chord(State::new(core));
            assert_eq!(state.core().level(), level);
            assert!(state.core().menu().is_none(), "chord must not open system menu");
            assert_eq!(
                choice_names(&state),
                vec!["Screenshot", "Screenshot window", "Start recording"]
            );
            let state = stick_step(state, 800);
            assert_eq!(
                state
                    .choice_list()
                    .and_then(tehuti_core::choice_list::List::selected),
                Some(Choice::Candidate(1))
            );
            let (state, effects) = tap_button(state, confirm);
            assert!(state.choice_list().is_none(), "confirm closes capture choice");
            assert!(
                effects.contains(&Effect::RunMenuCommand(Command::ScreenshotWindow)),
                "confirm runs selected capture command"
            );
            let state = open_capture_chord(state);
            let (state, cancel_effects) = tap_button(state, back);
            assert!(state.choice_list().is_none(), "back closes capture choice");
            assert!(
                !cancel_effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::RunMenuCommand(_))),
                "back runs no capture command"
            );
        }
    }

    #[test]
    fn pad_capture_chord_switches_recording_action_when_recording_is_active() {
        use tehuti_core::menu::Command;

        let mut state = State::default();
        state.set_recording(true);
        let state = open_capture_chord(state);
        assert_eq!(
            choice_names(&state),
            vec!["Screenshot", "Screenshot window", "Stop recording"]
        );
        let state = stick_step(stick_step(state, 800), 800);
        let (_, effects) = tap_button(state, ButtonName::South);
        assert!(
            effects.contains(&Effect::RunMenuCommand(Command::StopRecording)),
            "recording option runs StopRecording while recording is active"
        );
    }

    #[test]
    fn pad_capture_chord_works_over_a_passthrough_context() {
        use tehuti_core::menu::Command;

        let passthrough =
            tehuti_protocol::parse_context("app_id = 'example-game'\n[buttons]\n[pad]\npassthrough = true")
                .unwrap();
        let state = drive(
            State::new(tehuti_core::State::with_contexts(vec![passthrough])),
            Input::Event {
                event: Event::Focus {
                    app_id: Some("example-game".into()),
                    title: String::new(),
                },
                elapsed: Duration::ZERO,
            },
        )
        .0;
        let state = open_capture_chord(state);
        assert!(
            state.overlay_owns_pad(),
            "capture choice must own the pad over passthrough"
        );
        let (state, effects) = tap_button(state, ButtonName::South);
        assert!(
            effects.contains(&Effect::RunMenuCommand(Command::Screenshot)),
            "confirm runs Screenshot over passthrough"
        );
        assert!(
            !state.overlay_owns_pad(),
            "pad returns to passthrough after confirming"
        );
    }

    #[test]
    fn opening_capture_choice_releases_held_keys_and_allows_push_to_talk() {
        use tehuti_core::speech::{SpeechEffect, SpeechInput};
        use tehuti_protocol::{Action, KeyName};

        let context = tehuti_protocol::parse_context(
            "app_id = 'example-app'\n[buttons]\nwest = { key = 'enter', hold = true }",
        )
        .unwrap();
        let initial = State::new(tehuti_core::State::with_contexts(vec![context]));
        let (focused, _) = drive_with_plugin(
            initial,
            Input::Event {
                event: Event::Focus {
                    app_id: Some("example-app".into()),
                    title: String::new(),
                },
                elapsed: Duration::ZERO,
            },
            false,
        );
        let (ready, _) = drive_with_plugin(focused, Input::Speech(SpeechInput::Available), false);
        let (holding, down_effects) = drive_with_plugin(
            ready,
            Input::Event {
                event: Event::Button {
                    name: ButtonName::West,
                    state: ButtonState::Pressed,
                },
                elapsed: Duration::ZERO,
            },
            false,
        );
        assert!(
            down_effects.contains(&Effect::Apply(Action::KeyDown { key: KeyName::Enter })),
            "holding West must press the mapped key"
        );
        let (guide_down, _) = press_button(holding, ButtonName::Guide, ButtonState::Pressed);
        let (opened, open_effects) = press_button(guide_down, ButtonName::RightBumper, ButtonState::Pressed);
        assert!(
            open_effects.contains(&Effect::Apply(Action::KeyUp { key: KeyName::Enter })),
            "opening capture choice must release held keys"
        );
        let (bumper_up, _) = press_button(opened, ButtonName::RightBumper, ButtonState::Released);
        let (choice_open, _) = press_button(bumper_up, ButtonName::Guide, ButtonState::Released);
        let (talking, talk_effects) = press_button(choice_open, ButtonName::Record, ButtonState::Pressed);
        assert!(
            talk_effects.contains(&Effect::SpeechCommand(SpeechEffect::Start { session: 0 })),
            "push-to-talk must start while capture choice is open"
        );
        let (_, stop_effects) = press_button(talking, ButtonName::Record, ButtonState::Released);
        assert!(
            stop_effects.contains(&Effect::SpeechCommand(SpeechEffect::Stop {
                session: 0,
                transcribe: true,
            })),
            "releasing push-to-talk must transcribe while capture choice is open"
        );
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
