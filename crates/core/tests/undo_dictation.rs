#[cfg(test)]
mod undo_dictation {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Text, parse_context};

    fn terminal() -> State {
        let context = parse_context(
            "app_id = 'terminal'\nunmatched_speech = 'type'\n[buttons]\nwest = { undo_dictation = true }\n\
             [speech]\nconfirm = { key = 'enter' }\n'Scratch that' = { key = 'a' }\n",
        )
        .unwrap();
        step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("terminal".into()),
                title: String::new(),
            },
        )
        .0
    }

    fn say(state: State, words: &str) -> (State, Vec<Action>) {
        step(
            state,
            &Event::Speech {
                text: Text::new(words.into()).unwrap(),
            },
        )
    }

    fn press_x(state: State) -> (State, Vec<Action>) {
        step(
            state,
            &Event::Button {
                name: ButtonName::West,
                state: ButtonState::Pressed,
            },
        )
    }

    fn backspaces(count: usize) -> Vec<Action> {
        vec![
            Action::Key {
                key: KeyName::Backspace,
                mods: Vec::new(),
            };
            count
        ]
    }

    fn typed_length(actions: &[Action]) -> usize {
        match actions {
            [Action::Text { text }] => text.as_str().chars().count(),
            other => panic!("expected one typed phrase, got {other:?}"),
        }
    }

    proptest! {
        #[test]
        fn the_first_press_deletes_exactly_the_last_phrase_and_later_presses_one_character(
            phrase in "[a-zé]{1,8}( [a-z]{1,8}){0,4}",
            later in 1usize..4,
        ) {
            let (state, typed) = say(terminal(), &phrase);
            let length = typed_length(&typed);
            let (mut state, undone) = press_x(state);
            prop_assert_eq!(undone, backspaces(length));
            for _ in 0..later {
                let (next, actions) = press_x(state);
                prop_assert_eq!(actions, backspaces(1));
                state = next;
            }
        }

        #[test]
        fn anything_typed_since_the_phrase_makes_the_press_one_backspace(phrase in "[a-z]{1,8}") {
            let (state, _) = say(terminal(), &phrase);
            let (state, _) = say(state, "confirm");
            let (_, actions) = press_x(state);
            prop_assert_eq!(actions, backspaces(1));
        }

        #[test]
        fn only_the_latest_phrase_is_undone(first in "[a-z]{1,8}", second in "[a-z]{1,8}") {
            let (state, _) = say(terminal(), &first);
            let (state, typed) = say(state, &second);
            let (_, actions) = press_x(state);
            prop_assert_eq!(actions, backspaces(typed_length(&typed)));
        }
    }

    #[test]
    fn a_press_with_no_dictation_is_one_backspace() {
        assert_eq!(press_x(terminal()).1, backspaces(1));
    }

    #[test]
    fn scratch_that_does_what_the_button_does() {
        let (state, typed) = say(terminal(), "hello there");
        let (state, actions) = say(state, "Scratch that.");
        assert_eq!(actions, backspaces(typed_length(&typed)));
        assert_eq!(say(state, "Scratch that.").1, backspaces(1));
    }

    #[test]
    fn scratch_that_is_not_claimed_at_the_desktop_level() {
        let (state, _) = say(terminal(), "hello");
        let mut state = state;
        for button_state in [ButtonState::Pressed, ButtonState::Released] {
            state = step(
                state,
                &Event::Button {
                    name: ButtonName::Guide,
                    state: button_state,
                },
            )
            .0;
        }
        let (_, actions) = say(state, "Scratch that");
        assert_eq!(
            actions,
            vec![Action::Key {
                key: KeyName::A,
                mods: Vec::new()
            }],
            "the context mapping runs when core does not claim the phrase"
        );
    }

    #[test]
    fn an_active_plugin_receives_the_phrase_instead() {
        let (state, _) = say(terminal(), "hello");
        let (_, actions, routed) = tehuti_core::step_with_plugin(
            state,
            &Event::Speech {
                text: Text::new("Scratch that".into()).unwrap(),
            },
            true,
        );
        assert!(routed);
        assert_eq!(actions, Vec::<Action>::new());
    }

    #[test]
    fn plugins_cannot_emit_undo_dictation() {
        let context = parse_context("app_id = 'x'\n[buttons]\nwest = { undo_dictation = true }").unwrap();
        let undo = context.action_for(ButtonName::West).unwrap();
        let error = tehuti_core::plugin::validate_actions(vec![undo], &[]).unwrap_err();
        assert!(error.contains("reserved for the core"), "{error}");
    }

    fn release_x(state: State) -> (State, Vec<Action>) {
        step(
            state,
            &Event::Button {
                name: ButtonName::West,
                state: ButtonState::Released,
            },
        )
    }

    fn tick(state: State, millis: u64) -> (State, Vec<Action>) {
        step(
            state,
            &Event::Tick {
                elapsed: core::time::Duration::from_millis(millis),
            },
        )
    }

    fn held_backspaces(mut state: State, slices: &[u64]) -> usize {
        let mut count = 0;
        for millis in slices {
            let (next, actions) = tick(state, *millis);
            assert!(
                actions.iter().all(|action| *action == backspaces(1)[0]),
                "{actions:?}"
            );
            count += actions.len();
            state = next;
        }
        count
    }

    #[test]
    fn holding_the_button_repeats_backspace_after_the_delay() {
        let (state, _) = press_x(terminal());
        let (state, early) = tick(state, 100);
        assert_eq!(early, Vec::new());
        let (state, repeated) = tick(state, 200);
        assert_eq!(repeated, backspaces(1));
        let (state, _) = release_x(state);
        assert_eq!(tick(state, 2000).1, Vec::new());
    }

    #[test]
    fn a_button_that_is_not_undo_does_not_repeat() {
        let state = step(
            terminal(),
            &Event::Button {
                name: ButtonName::East,
                state: ButtonState::Pressed,
            },
        )
        .0;
        assert_eq!(tick(state, 2000).1, Vec::new());
    }

    proptest! {
        #[test]
        fn the_repeat_count_does_not_depend_on_how_ticks_are_sliced(
            slices in proptest::collection::vec(1u64..100, 1..20),
        ) {
            let total: u64 = slices.iter().sum();
            let (state, _) = press_x(terminal());
            let sliced = held_backspaces(state.clone(), &slices);
            let whole = held_backspaces(state, &[total]);
            prop_assert_eq!(sliced, whole);
        }

        #[test]
        fn a_long_hold_repeats_and_a_longer_one_repeats_more(extra in 1u64..2000) {
            let (state, _) = press_x(terminal());
            let short = held_backspaces(state.clone(), &[1000]);
            let long = held_backspaces(state, &[1000 + extra]);
            prop_assert!(short >= 1 && long >= short);
        }

        #[test]
        fn nothing_repeats_before_the_delay(millis in 1u64..299) {
            let (state, _) = press_x(terminal());
            prop_assert_eq!(held_backspaces(state, &[millis]), 0);
        }
    }

    #[test]
    fn shipped_terminal_contexts_bind_undo_to_x() {
        for source in [
            include_str!("../../../dist/contexts/kitty/context.toml"),
            include_str!("../../../examples/foot/context.toml"),
        ] {
            let context = parse_context(source).unwrap();
            assert_eq!(context.action_for(ButtonName::West), Some(Action::UndoDictation));
        }
    }
}
