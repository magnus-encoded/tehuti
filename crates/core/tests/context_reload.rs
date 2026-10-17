#[cfg(test)]
mod context_reload {
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Text, parse_context};

    fn context(key: &str) -> tehuti_protocol::Context {
        parse_context(&format!(
            "app_id = 'terminal'\n[buttons]\neast = {{ key = '{key}' }}\n[speech]\nconfirm = {{ key = '{key}' }}"
        ))
        .unwrap()
    }

    fn press(name: ButtonName) -> Event {
        Event::Button {
            name,
            state: ButtonState::Pressed,
        }
    }

    #[test]
    fn reload_swaps_mappings_and_keeps_focus() {
        let (mut state, _) = step(
            State::with_contexts(vec![context("enter")]),
            &Event::Focus {
                app_id: Some("terminal".into()),
                title: String::new(),
            },
        );
        state.replace_contexts(vec![context("tab")]);
        let (state, button) = step(state, &press(ButtonName::East));
        assert_eq!(
            button,
            vec![Action::Key {
                key: KeyName::Tab,
                mods: Vec::new()
            }]
        );
        let (_, speech) = step(
            state,
            &Event::Speech {
                text: Text::new("confirm".into()).unwrap(),
            },
        );
        assert_eq!(
            speech,
            vec![Action::Key {
                key: KeyName::Tab,
                mods: Vec::new()
            }]
        );
    }

    #[test]
    fn reload_keeps_held_buttons() {
        let (mut held, _) = step(
            State::with_contexts(vec![context("enter")]),
            &press(ButtonName::Guide),
        );
        let untouched = held.clone();
        held.replace_contexts(vec![context("tab")]);
        let finish = |state: State| {
            step(
                state,
                &Event::Tick {
                    elapsed: core::time::Duration::from_secs(3),
                },
            )
            .0
            .menu()
            .cloned()
        };
        let without_reload = finish(untouched);
        assert!(without_reload.is_some(), "the held hold opens the menu");
        assert_eq!(finish(held), without_reload);
    }
}
