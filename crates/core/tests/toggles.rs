//! Mode toggles: voice destinations, Toggles settings page, active bar chips, and chip focus.

#[cfg(test)]
mod toggles {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        Level,
        bar::{Segment, Tone},
        dispatch::{Category, Effect, Input, Page, State, step},
        toggles::{BuiltinMode, Owner},
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, LayoutAction, Text, Toggle};

    fn answer_effect(effect: Effect, state: &mut State, trace: &mut Vec<Effect>) -> Option<Input> {
        if let Effect::Handle(event) = effect {
            let (handled, nested) = run(
                core::mem::take(state),
                Input::Event {
                    event,
                    elapsed: Duration::ZERO,
                },
            );
            *state = handled;
            trace.extend(nested);
            return None;
        }
        if matches!(effect, Effect::ReadClock) {
            Some(Input::ClockRead(Duration::from_millis(16)))
        } else if matches!(effect, Effect::QueryPlugin(_)) {
            Some(Input::PluginActive(false))
        } else if matches!(effect, Effect::DispatchPlugin { .. }) {
            Some(Input::PluginRequest(None))
        } else if matches!(effect, Effect::Supervise) {
            Some(Input::Supervised(Vec::new()))
        } else if matches!(effect, Effect::ConfirmSecret(_)) {
            Some(Input::SecretConfirmation(false))
        } else if matches!(effect, Effect::SpeechCommand(_)) {
            Some(Input::SpeechSent(true))
        } else if matches!(effect, Effect::PollSpeech(_)) {
            Some(Input::SpeechPolled(Vec::new()))
        } else {
            None
        }
    }

    fn run(mut state: State, input: Input) -> (State, Vec<Effect>) {
        let depth = state.busy();
        let (initial, mut effects) = step(state, input);
        state = initial;
        let mut trace = Vec::new();
        loop {
            let mut reply = Input::Resume;
            for effect in effects {
                trace.push(effect.clone());
                if let Some(answer) = answer_effect(effect, &mut state, &mut trace) {
                    reply = answer;
                }
            }
            if state.busy() <= depth {
                return (state, trace);
            }
            let (next_state, next_effects) = step(state, reply);
            state = next_state;
            effects = next_effects;
        }
    }

    fn say(state: State, phrase: &str) -> (State, Vec<Effect>) {
        run(
            state,
            Input::Event {
                event: Event::Speech {
                    text: Text::new(phrase.into()).unwrap(),
                },
                elapsed: Duration::ZERO,
            },
        )
    }

    fn press(state: State, name: ButtonName) -> (State, Vec<Effect>) {
        let (after_press, mut first) = run(
            state,
            Input::Event {
                event: Event::Button {
                    name,
                    state: ButtonState::Pressed,
                },
                elapsed: Duration::ZERO,
            },
        );
        let (after_release, second) = run(
            after_press,
            Input::Event {
                event: Event::Button {
                    name,
                    state: ButtonState::Released,
                },
                elapsed: Duration::ZERO,
            },
        );
        first.extend(second);
        (after_release, first)
    }

    fn mode_segments(state: &State) -> Vec<Segment> {
        state
            .core()
            .bar()
            .segments()
            .into_iter()
            .filter(|segment| matches!(segment.tone, Tone::Mode | Tone::ModeFocused))
            .collect()
    }

    fn open_toggles_page(state: State) -> State {
        let (mut current, _) = say(state, "Pop the hood.");
        for _ in 0..3 {
            current = press(current, ButtonName::DpadDown).0;
        }
        assert_eq!(current.settings().category, Category::Toggles);
        current = press(current, ButtonName::South).0;
        assert_eq!(current.settings().page, Page::Toggles);
        current
    }

    #[test]
    fn voice_sets_named_target_states_and_never_toggles_without_a_state() {
        let initial = State::default();
        assert!(mode_segments(&initial).is_empty(), "no chips show initially");

        let (after_bare, bare_effects) = say(initial, "stay awake");
        assert!(
            !after_bare.toggles().builtin_active(BuiltinMode::StayAwake),
            "bare mode name does not flip"
        );
        assert!(
            mode_segments(&after_bare).is_empty(),
            "bare mode name adds no chip"
        );
        assert!(
            !bare_effects
                .iter()
                .any(|effect| matches!(effect, Effect::ModeToggleChanged { .. })),
            "bare mode name emits no toggle effect"
        );

        let (after_on, on_effects) = say(after_bare, "Stay awake on.");
        assert!(
            after_on.toggles().builtin_active(BuiltinMode::StayAwake),
            "stay awake turns on"
        );
        assert!(
            on_effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::StayAwake),
                name: "stay awake".into(),
                active: true,
            }),
            "turning stay awake on emits ModeToggleChanged"
        );
        assert_eq!(
            mode_segments(&after_on),
            vec![Segment {
                text: "Stay awake".into(),
                tone: Tone::Mode,
            }]
        );

        let (after_repeat, repeat_effects) = say(after_on, "stay awake on");
        assert!(
            after_repeat.toggles().builtin_active(BuiltinMode::StayAwake),
            "idempotent stay awake on"
        );
        assert!(
            !repeat_effects
                .iter()
                .any(|effect| matches!(effect, Effect::ModeToggleChanged { .. })),
            "repeating an already-active target emits no duplicate effect"
        );
        assert!(
            !repeat_effects
                .iter()
                .any(|effect| matches!(effect, Effect::Apply(Action::Notice { .. }))),
            "repeating an already-active target is consumed"
        );
    }

    #[test]
    fn voice_shows_and_hides_multiple_mode_chips_only_while_active() {
        let (awake_only, _) = say(State::default(), "stay awake on");
        let (both_active, mute_on_effects) = say(awake_only, "microphone mute on");
        assert!(
            both_active.toggles().builtin_active(BuiltinMode::MicrophoneMute),
            "mic mute turns on"
        );
        assert!(
            mute_on_effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::MicrophoneMute),
                name: "microphone mute".into(),
                active: true,
            }),
            "mic mute emits ModeToggleChanged"
        );
        let both_texts: Vec<_> = mode_segments(&both_active)
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(both_texts, vec!["Stay awake", "Microphone mute"]);

        let (after_off, mute_off_effects) = say(both_active, "microphone mute off");
        assert!(
            !after_off.toggles().builtin_active(BuiltinMode::MicrophoneMute),
            "mic mute turns off"
        );
        assert!(
            mute_off_effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::MicrophoneMute),
                name: "microphone mute".into(),
                active: false,
            }),
            "mic mute off emits ModeToggleChanged"
        );
        let remaining_texts: Vec<_> = mode_segments(&after_off)
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(remaining_texts, vec!["Stay awake"]);
    }

    #[test]
    fn toggles_page_lists_every_mode_and_dpad_steps_while_confirm_flips() {
        let plugin_toggle = Toggle::new("night light", false).unwrap();
        let (seeded, _) = run(State::default(), Input::PluginToggles(vec![(4, plugin_toggle)]));
        let opened = open_toggles_page(seeded);

        let names: Vec<&str> = opened
            .toggles()
            .items()
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["stay awake", "microphone mute", "do not disturb", "night light"]
        );

        let (stepped, move_effects) = press(opened, ButtonName::DpadRight);
        assert_eq!(stepped.settings().toggles_focus, 1);
        assert!(
            !move_effects
                .iter()
                .any(|effect| matches!(effect, Effect::ModeToggleChanged { .. })),
            "moving focus never flips a toggle"
        );

        let on_plugin = press(press(stepped, ButtonName::DpadRight).0, ButtonName::DpadRight).0;
        assert_eq!(on_plugin.settings().toggles_focus, 3);
        let (flipped_on, flip_effects) = press(on_plugin, ButtonName::South);
        assert!(
            flipped_on.toggles().is_active("night light"),
            "confirm flips plugin toggle on"
        );
        assert!(
            flip_effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Plugin(4),
                name: "night light".into(),
                active: true,
            }),
            "plugin toggle on emits ModeToggleChanged"
        );
        assert_eq!(
            mode_segments(&flipped_on),
            vec![Segment {
                text: "Night light".into(),
                tone: Tone::Mode,
            }]
        );

        let (flipped_off, _) = press(flipped_on, ButtonName::South);
        assert!(
            !flipped_off.toggles().is_active("night light"),
            "confirm flips plugin toggle off"
        );
        assert!(mode_segments(&flipped_off).is_empty(), "chip disappears when off");
    }

    #[test]
    fn late_arriving_plugin_toggles_join_end_without_moving_focus() {
        let first = Toggle::new("night light", false).unwrap();
        let (seeded, _) = run(State::default(), Input::PluginToggles(vec![(4, first.clone())]));
        let mut opened = open_toggles_page(seeded);
        for _ in 0..3 {
            opened = press(opened, ButtonName::DpadRight).0;
        }
        assert_eq!(opened.settings().toggles_focus, 3);
        let late = Toggle::new("stream mode", true).unwrap();
        let (updated, _) = run(opened, Input::PluginToggles(vec![(4, first), (7, late)]));
        assert_eq!(updated.settings().toggles_focus, 3);
        assert_eq!(updated.toggles().get(3).unwrap().name, "night light");
        assert_eq!(updated.toggles().get(4).unwrap().name, "stream mode");
    }

    #[test]
    fn focusing_an_active_bar_chip_and_pressing_confirm_turns_it_off() {
        let (with_awake, _) = say(State::default(), "stay awake on");
        let (with_both, _) = say(with_awake, "do not disturb on");

        let (columns, _) = press(with_both, ButtonName::Guide);
        assert_eq!(columns.core().level(), Level::Columns);
        assert_eq!(columns.bar_chip_focus(), None);

        let (focused_first, _) = press(columns, ButtonName::DpadDown);
        assert_eq!(focused_first.bar_chip_focus(), Some(0));
        assert_eq!(mode_segments(&focused_first)[0].tone, Tone::ModeFocused);
        assert_eq!(mode_segments(&focused_first)[1].tone, Tone::Mode);

        let (focused_second, _) = press(focused_first, ButtonName::DpadRight);
        assert_eq!(focused_second.bar_chip_focus(), Some(1));
        assert_eq!(mode_segments(&focused_second)[1].tone, Tone::ModeFocused);

        let (after_first_off, first_off_effects) = press(focused_second, ButtonName::South);
        assert!(
            !after_first_off
                .toggles()
                .builtin_active(BuiltinMode::DoNotDisturb),
            "confirm turns focused chip off"
        );
        assert!(
            after_first_off.toggles().builtin_active(BuiltinMode::StayAwake),
            "other active chip stays on"
        );
        assert!(
            first_off_effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::DoNotDisturb),
                name: "do not disturb".into(),
                active: false,
            }),
            "turning off focused chip emits ModeToggleChanged"
        );
        assert_eq!(after_first_off.bar_chip_focus(), Some(0));

        let (after_last_off, last_off_effects) = press(after_first_off, ButtonName::South);
        assert!(
            !after_last_off.toggles().builtin_active(BuiltinMode::StayAwake),
            "confirm turns last chip off"
        );
        assert_eq!(after_last_off.bar_chip_focus(), None);
        assert!(mode_segments(&after_last_off).is_empty(), "all active chips gone");
        assert!(
            !last_off_effects
                .iter()
                .any(|effect| matches!(effect, Effect::Apply(_))),
            "bar chip confirm never leaks Enter to the app"
        );
    }

    #[test]
    fn guide_dpad_chord_still_moves_columns_even_with_active_mode_chips() {
        let (with_awake, _) = say(State::default(), "stay awake on");
        let (columns, _) = press(with_awake, ButtonName::Guide);
        assert_eq!(columns.core().level(), Level::Columns);
        let (guide_down, _) = run(
            columns,
            Input::Event {
                event: Event::Button {
                    name: ButtonName::Guide,
                    state: ButtonState::Pressed,
                },
                elapsed: Duration::ZERO,
            },
        );
        let (after_chord, chord_effects) = press(guide_down, ButtonName::DpadDown);
        assert_eq!(after_chord.bar_chip_focus(), None);
        assert!(
            chord_effects.contains(&Effect::Apply(Action::Layout {
                action: LayoutAction::MoveColumnToWorkspaceDown,
            })),
            "Guide+D-pad chord still moves columns"
        );
    }

    #[test]
    fn plugin_exit_clears_its_toggles_and_active_bar_chips() {
        let toggle = Toggle::new("night light", true).unwrap();
        let (registered, _) = run(State::default(), Input::PluginToggles(vec![(2, toggle)]));
        assert_eq!(
            mode_segments(&registered),
            vec![Segment {
                text: "Night light".into(),
                tone: Tone::Mode,
            }]
        );
        let (after_exit, _) = run(registered, Input::PluginExited { plugin: 2 });
        assert!(
            !after_exit.toggles().is_active("night light"),
            "exited plugin toggle is removed"
        );
        assert!(
            mode_segments(&after_exit).is_empty(),
            "exited plugin chip disappears"
        );
    }

    #[derive(Debug, Clone, Copy)]
    enum StepAction {
        Voice(usize, bool),
        OpenAndFlip(usize),
        ToggleLevel,
        FocusBar,
        StepBar(bool),
        TurnOffFocusedChip,
        ExitBar,
        SyncPlugin(bool),
    }

    fn action_strategy() -> impl Strategy<Value = StepAction> {
        prop_oneof![
            (0usize..4, any::<bool>()).prop_map(|(index, enable)| StepAction::Voice(index, enable)),
            (0usize..4).prop_map(StepAction::OpenAndFlip),
            Just(StepAction::ToggleLevel),
            Just(StepAction::FocusBar),
            any::<bool>().prop_map(StepAction::StepBar),
            Just(StepAction::TurnOffFocusedChip),
            Just(StepAction::ExitBar),
            any::<bool>().prop_map(StepAction::SyncPlugin),
        ]
    }

    fn apply_action(state: State, action: StepAction) -> State {
        let names = ["stay awake", "microphone mute", "do not disturb", "night light"];
        match action {
            StepAction::Voice(index, enable) => {
                let phrase = format!("{} {}", names[index], if enable { "on" } else { "off" });
                say(state, &phrase).0
            }
            StepAction::OpenAndFlip(index) => {
                let mut current = if state.settings().open {
                    state
                } else {
                    open_toggles_page(state)
                };
                if current.settings().page == Page::Toggles {
                    for _ in 0..4 {
                        current = press(current, ButtonName::DpadLeft).0;
                    }
                    for _ in 0..index {
                        current = press(current, ButtonName::DpadRight).0;
                    }
                    current = press(current, ButtonName::South).0;
                    current = press(current, ButtonName::East).0;
                    current = press(current, ButtonName::East).0;
                }
                current
            }
            StepAction::ToggleLevel => press(state, ButtonName::Guide).0,
            StepAction::FocusBar => press(state, ButtonName::DpadDown).0,
            StepAction::StepBar(right) => {
                press(
                    state,
                    if right {
                        ButtonName::DpadRight
                    } else {
                        ButtonName::DpadLeft
                    },
                )
                .0
            }
            StepAction::TurnOffFocusedChip => press(state, ButtonName::South).0,
            StepAction::ExitBar => press(state, ButtonName::East).0,
            StepAction::SyncPlugin(present) => {
                let list = if present {
                    vec![(1, Toggle::new("night light", false).unwrap())]
                } else {
                    Vec::new()
                };
                run(state, Input::PluginToggles(list)).0
            }
        }
    }

    proptest! {
        #[test]
        fn bar_chips_always_match_active_modes_and_voice_names(
            actions in prop::collection::vec(action_strategy(), 1..40),
        ) {
            let mut state = State::default();
            for action in actions {
                state = apply_action(state, action);
                let active: Vec<&str> = state
                    .toggles()
                    .active_items()
                    .into_iter()
                    .map(|item| item.name.as_str())
                    .collect();
                let chips = mode_segments(&state);
                let chip_names: Vec<String> = chips
                    .iter()
                    .map(|segment| tehuti_protocol::normalize_phrase(&segment.text))
                    .collect();
                prop_assert_eq!(chip_names, active);
                if let Some(focused) = state.bar_chip_focus() {
                    prop_assert!(focused < chips.len());
                    prop_assert_eq!(state.core().level(), Level::Columns);
                    prop_assert_eq!(chips[focused].tone, Tone::ModeFocused);
                } else {
                    prop_assert!(chips.iter().all(|segment| segment.tone == Tone::Mode));
                }
            }
        }
    }
}
