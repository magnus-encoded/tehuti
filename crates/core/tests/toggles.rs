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
        let (next, mut effects) = step(state, input);
        state = next;
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
            let (next, next_effects) = step(state, reply);
            state = next;
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
        let (state, mut first) = run(
            state,
            Input::Event {
                event: Event::Button {
                    name,
                    state: ButtonState::Pressed,
                },
                elapsed: Duration::ZERO,
            },
        );
        let (state, second) = run(
            state,
            Input::Event {
                event: Event::Button {
                    name,
                    state: ButtonState::Released,
                },
                elapsed: Duration::ZERO,
            },
        );
        first.extend(second);
        (state, first)
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
        let (mut state, _) = say(state, "Pop the hood.");
        for _ in 0..3 {
            state = press(state, ButtonName::DpadDown).0;
        }
        assert_eq!(state.settings().category, Category::Toggles);
        state = press(state, ButtonName::South).0;
        assert_eq!(state.settings().page, Page::Toggles);
        state
    }

    #[test]
    fn voice_sets_named_target_states_and_never_toggles_without_a_state() {
        let state = State::default();
        assert!(mode_segments(&state).is_empty(), "no chips show initially");

        let (state, effects) = say(state, "stay awake");
        assert!(
            !state.toggles().builtin_active(BuiltinMode::StayAwake),
            "bare mode name does not flip"
        );
        assert!(mode_segments(&state).is_empty(), "bare mode name adds no chip");
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::ModeToggleChanged { .. })),
            "bare mode name emits no toggle effect"
        );

        let (state, effects) = say(state, "Stay awake on.");
        assert!(
            state.toggles().builtin_active(BuiltinMode::StayAwake),
            "stay awake turns on"
        );
        assert!(
            effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::StayAwake),
                name: "stay awake".into(),
                active: true,
            }),
            "turning stay awake on emits ModeToggleChanged"
        );
        assert_eq!(
            mode_segments(&state),
            vec![Segment {
                text: "Stay awake".into(),
                tone: Tone::Mode,
            }]
        );

        let (state, effects) = say(state, "stay awake on");
        assert!(
            state.toggles().builtin_active(BuiltinMode::StayAwake),
            "idempotent stay awake on"
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::ModeToggleChanged { .. })),
            "repeating an already-active target emits no duplicate effect"
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Apply(Action::Notice { .. }))),
            "repeating an already-active target is consumed"
        );
    }

    #[test]
    fn voice_shows_and_hides_multiple_mode_chips_only_while_active() {
        let (state, _) = say(State::default(), "stay awake on");
        let (state, effects) = say(state, "microphone mute on");
        assert!(
            state.toggles().builtin_active(BuiltinMode::MicrophoneMute),
            "mic mute turns on"
        );
        assert!(
            effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::MicrophoneMute),
                name: "microphone mute".into(),
                active: true,
            }),
            "mic mute emits ModeToggleChanged"
        );
        let texts: Vec<_> = mode_segments(&state)
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(texts, vec!["Stay awake", "Microphone mute"]);

        let (state, effects) = say(state, "microphone mute off");
        assert!(
            !state.toggles().builtin_active(BuiltinMode::MicrophoneMute),
            "mic mute turns off"
        );
        assert!(
            effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::MicrophoneMute),
                name: "microphone mute".into(),
                active: false,
            }),
            "mic mute off emits ModeToggleChanged"
        );
        let texts: Vec<_> = mode_segments(&state)
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(texts, vec!["Stay awake"]);
    }

    #[test]
    fn toggles_page_lists_every_mode_and_dpad_steps_while_confirm_flips() {
        let plugin_toggle = Toggle::new("night light", false).unwrap();
        let (state, _) = run(State::default(), Input::PluginToggles(vec![(4, plugin_toggle)]));
        let state = open_toggles_page(state);

        let names: Vec<&str> = state
            .toggles()
            .items()
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["stay awake", "microphone mute", "do not disturb", "night light"]
        );

        let (state, effects) = press(state, ButtonName::DpadRight);
        assert_eq!(state.settings().toggles_focus, 1);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::ModeToggleChanged { .. })),
            "moving focus never flips a toggle"
        );

        let state = press(press(state, ButtonName::DpadRight).0, ButtonName::DpadRight).0;
        assert_eq!(state.settings().toggles_focus, 3);
        let (state, effects) = press(state, ButtonName::South);
        assert!(
            state.toggles().is_active("night light"),
            "confirm flips plugin toggle on"
        );
        assert!(
            effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Plugin(4),
                name: "night light".into(),
                active: true,
            }),
            "plugin toggle on emits ModeToggleChanged"
        );
        assert_eq!(
            mode_segments(&state),
            vec![Segment {
                text: "Night light".into(),
                tone: Tone::Mode,
            }]
        );

        let (state, _) = press(state, ButtonName::South);
        assert!(
            !state.toggles().is_active("night light"),
            "confirm flips plugin toggle off"
        );
        assert!(mode_segments(&state).is_empty(), "chip disappears when off");
    }

    #[test]
    fn late_arriving_plugin_toggles_join_end_without_moving_focus() {
        let first = Toggle::new("night light", false).unwrap();
        let (state, _) = run(State::default(), Input::PluginToggles(vec![(4, first.clone())]));
        let mut state = open_toggles_page(state);
        for _ in 0..3 {
            state = press(state, ButtonName::DpadRight).0;
        }
        assert_eq!(state.settings().toggles_focus, 3);
        let late = Toggle::new("stream mode", true).unwrap();
        let (state, _) = run(state, Input::PluginToggles(vec![(4, first), (7, late)]));
        assert_eq!(state.settings().toggles_focus, 3);
        assert_eq!(state.toggles().get(3).unwrap().name, "night light");
        assert_eq!(state.toggles().get(4).unwrap().name, "stream mode");
    }

    #[test]
    fn focusing_an_active_bar_chip_and_pressing_confirm_turns_it_off() {
        let (state, _) = say(State::default(), "stay awake on");
        let (state, _) = say(state, "do not disturb on");

        let (state, _) = press(state, ButtonName::Guide);
        assert_eq!(state.core().level(), Level::Columns);
        assert_eq!(state.bar_chip_focus(), None);

        let (state, _) = press(state, ButtonName::DpadDown);
        assert_eq!(state.bar_chip_focus(), Some(0));
        assert_eq!(mode_segments(&state)[0].tone, Tone::ModeFocused);
        assert_eq!(mode_segments(&state)[1].tone, Tone::Mode);

        let (state, _) = press(state, ButtonName::DpadRight);
        assert_eq!(state.bar_chip_focus(), Some(1));
        assert_eq!(mode_segments(&state)[1].tone, Tone::ModeFocused);

        let (state, effects) = press(state, ButtonName::South);
        assert!(
            !state.toggles().builtin_active(BuiltinMode::DoNotDisturb),
            "confirm turns focused chip off"
        );
        assert!(
            state.toggles().builtin_active(BuiltinMode::StayAwake),
            "other active chip stays on"
        );
        assert!(
            effects.contains(&Effect::ModeToggleChanged {
                owner: Owner::Builtin(BuiltinMode::DoNotDisturb),
                name: "do not disturb".into(),
                active: false,
            }),
            "turning off focused chip emits ModeToggleChanged"
        );
        assert_eq!(state.bar_chip_focus(), Some(0));

        let (state, effects) = press(state, ButtonName::South);
        assert!(
            !state.toggles().builtin_active(BuiltinMode::StayAwake),
            "confirm turns last chip off"
        );
        assert_eq!(state.bar_chip_focus(), None);
        assert!(mode_segments(&state).is_empty(), "all active chips gone");
        assert!(
            !effects.iter().any(|effect| matches!(effect, Effect::Apply(_))),
            "bar chip confirm never leaks Enter to the app"
        );
    }

    #[test]
    fn guide_dpad_chord_still_moves_columns_even_with_active_mode_chips() {
        let (state, _) = say(State::default(), "stay awake on");
        let (state, _) = press(state, ButtonName::Guide);
        assert_eq!(state.core().level(), Level::Columns);
        let (state, _) = run(
            state,
            Input::Event {
                event: Event::Button {
                    name: ButtonName::Guide,
                    state: ButtonState::Pressed,
                },
                elapsed: Duration::ZERO,
            },
        );
        let (state, effects) = press(state, ButtonName::DpadDown);
        assert_eq!(state.bar_chip_focus(), None);
        assert!(
            effects.contains(&Effect::Apply(Action::Layout {
                action: LayoutAction::MoveColumnToWorkspaceDown,
            })),
            "Guide+D-pad chord still moves columns"
        );
    }

    #[test]
    fn plugin_exit_clears_its_toggles_and_active_bar_chips() {
        let toggle = Toggle::new("night light", true).unwrap();
        let (state, _) = run(State::default(), Input::PluginToggles(vec![(2, toggle)]));
        assert_eq!(
            mode_segments(&state),
            vec![Segment {
                text: "Night light".into(),
                tone: Tone::Mode,
            }]
        );
        let (state, _) = run(state, Input::PluginExited { plugin: 2 });
        assert!(
            !state.toggles().is_active("night light"),
            "exited plugin toggle is removed"
        );
        assert!(mode_segments(&state).is_empty(), "exited plugin chip disappears");
    }

    #[derive(Debug, Clone)]
    enum StepOp {
        Voice(usize, bool),
        OpenAndFlip(usize),
        ToggleLevel,
        FocusBar,
        StepBar(bool),
        TurnOffFocusedChip,
        ExitBar,
        SyncPlugin(bool),
    }

    fn op_strategy() -> impl Strategy<Value = StepOp> {
        prop_oneof![
            (0usize..4, any::<bool>()).prop_map(|(index, on)| StepOp::Voice(index, on)),
            (0usize..4).prop_map(StepOp::OpenAndFlip),
            Just(StepOp::ToggleLevel),
            Just(StepOp::FocusBar),
            any::<bool>().prop_map(StepOp::StepBar),
            Just(StepOp::TurnOffFocusedChip),
            Just(StepOp::ExitBar),
            any::<bool>().prop_map(StepOp::SyncPlugin),
        ]
    }

    fn apply_op(state: State, op: StepOp) -> State {
        let names = ["stay awake", "microphone mute", "do not disturb", "night light"];
        match op {
            StepOp::Voice(index, on) => {
                let phrase = format!("{} {}", names[index], if on { "on" } else { "off" });
                say(state, &phrase).0
            }
            StepOp::OpenAndFlip(index) => {
                let mut next = if state.settings().open {
                    state
                } else {
                    open_toggles_page(state)
                };
                if next.settings().page == Page::Toggles {
                    for _ in 0..4 {
                        next = press(next, ButtonName::DpadLeft).0;
                    }
                    for _ in 0..index {
                        next = press(next, ButtonName::DpadRight).0;
                    }
                    next = press(next, ButtonName::South).0;
                    next = press(next, ButtonName::East).0;
                    next = press(next, ButtonName::East).0;
                }
                next
            }
            StepOp::ToggleLevel => press(state, ButtonName::Guide).0,
            StepOp::FocusBar => press(state, ButtonName::DpadDown).0,
            StepOp::StepBar(right) => {
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
            StepOp::TurnOffFocusedChip => press(state, ButtonName::South).0,
            StepOp::ExitBar => press(state, ButtonName::East).0,
            StepOp::SyncPlugin(present) => {
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
        fn bar_chips_always_match_active_modes_and_voice_names(ops in prop::collection::vec(op_strategy(), 1..40)) {
            let mut state = State::default();
            for op in ops {
                state = apply_op(state, op);
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
