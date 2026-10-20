#[cfg(test)]
mod global_context {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Text, parse_context};

    const GLOBAL: &str = "app_id = '*'\n[buttons]\nwest = { key = 'backspace' }\n[speech]\n'Open Atlas.' = { key = 'enter' }\n'Clear.' = { key = 'escape' }";

    fn state(focused: Option<&str>) -> State {
        let browser = parse_context(
            "app_id = 'browser'\nunmatched_speech = 'type'\n[buttons]\n[speech]\n'Clear.' = { key = 'space' }",
        )
        .unwrap();
        let global = parse_context(GLOBAL).unwrap();
        let (state, _) = step(
            State::with_contexts(vec![browser, global]),
            &Event::Focus {
                app_id: focused.map(Into::into),
                title: String::new(),
            },
        );
        state
    }

    fn speak(state: State, text: &str) -> Vec<Action> {
        step(
            state,
            &Event::Speech {
                text: Text::new(text.into()).unwrap(),
            },
        )
        .1
    }

    #[test]
    fn global_phrase_beats_focused_typing_and_focused_phrase_wins_conflicts() {
        assert_eq!(
            speak(state(Some("browser")), "open atlas"),
            vec![Action::Key {
                key: KeyName::Enter,
                mods: Vec::new()
            }]
        );
        assert_eq!(
            speak(state(Some("browser")), "Clear."),
            vec![Action::Key {
                key: KeyName::Space,
                mods: Vec::new()
            }]
        );
        assert!(matches!(
            speak(state(Some("browser")), "hello").as_slice(),
            [Action::Text { .. }]
        ));
    }

    #[test]
    fn global_buttons_apply_where_the_focused_context_has_none() {
        let (_, actions) = step(
            state(Some("browser")),
            &Event::Button {
                name: ButtonName::West,
                state: ButtonState::Pressed,
            },
        );
        assert_eq!(
            actions,
            vec![Action::Key {
                key: KeyName::Backspace,
                mods: Vec::new()
            }]
        );
    }

    proptest! {
        #[test]
        fn global_phrase_runs_under_any_focus(
            focused in proptest::option::of("[a-z]{1,8}"),
            spoken in prop::sample::select(vec!["Open Atlas.", "open atlas", "OPEN ATLAS!"]),
        ) {
            prop_assert_eq!(
                speak(state(focused.as_deref()), spoken),
                vec![Action::Key { key: KeyName::Enter, mods: Vec::new() }]
            );
        }
    }
}
