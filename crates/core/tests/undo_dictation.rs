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
}
