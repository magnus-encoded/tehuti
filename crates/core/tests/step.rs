//! Properties of the core step over generated event series.

#[cfg(test)]
mod step {
    use core::fmt::Write;

    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName};

    fn run(events: &[Event]) -> Vec<Action> {
        let mut state = State::new();
        let mut actions = Vec::new();
        for event in events {
            let (next, emitted) = step(state, event);
            state = next;
            actions.extend(emitted);
        }
        actions
    }

    const A_PRESSED: Event = Event::Button {
        name: ButtonName::South,
        state: ButtonState::Pressed,
    };

    proptest! {
        #[test]
        fn step_returns_for_any_event_series(events in prop::collection::vec(any::<Event>(), 0..200)) {
            let _actions = run(&events);
        }

        #[test]
        fn each_a_press_sends_one_enter_and_nothing_else_reaches_the_client(
            events in prop::collection::vec(any::<Event>(), 0..200),
        ) {
            let a_presses = events.iter().filter(|event| **event == A_PRESSED).count();
            prop_assert_eq!(run(&events), vec![Action::Key { key: KeyName::Enter }; a_presses]);
        }
    }

    fn generated_context(
        app_id: &str,
        mappings: &std::collections::HashMap<ButtonName, KeyName>,
    ) -> tehuti_protocol::Context {
        let mut source = format!("app_id = '{app_id}'\n[buttons]\n");
        for (button, key) in mappings {
            write!(
                source,
                "[buttons.{}]\nkey = '{}'\n",
                format!("{button:?}").to_lowercase(),
                format!("{key:?}").to_lowercase()
            )
            .unwrap();
        }
        tehuti_protocol::parse_context(&source).unwrap()
    }

    proptest! {
        #[test]
        fn routing_stops_at_the_first_consuming_layer(
            mappings in prop::collection::hash_map(prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::North, ButtonName::West]), any::<KeyName>(), 0..=4),
            other_mappings in prop::collection::hash_map(prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::North, ButtonName::West]), any::<KeyName>(), 0..=4),
            focused in prop::option::of(prop::sample::select(vec!["test", "other", "unmatched"])),
            name in any::<ButtonName>(),
            button_state in any::<ButtonState>(),
        ) {
            let state = State::with_contexts(vec![generated_context("test", &mappings), generated_context("other", &other_mappings)]);
            let (state, focus_actions) = step(state, &Event::Focus { app_id: focused.map(str::to_owned), title: String::new() });
            prop_assert!(focus_actions.is_empty());
            let (_, actions) = step(state, &Event::Button { name, state: button_state });
            if button_state == ButtonState::Pressed && name == ButtonName::South {
                prop_assert_eq!(actions, vec![Action::Key { key: KeyName::Enter }], "global consumption excludes context and fallback output");
            } else if button_state == ButtonState::Pressed {
                let mapping = match focused {
                    Some("test") => mappings.get(&name),
                    Some("other") => other_mappings.get(&name),
                    _ => None,
                };
                if let Some(key) = mapping {
                    prop_assert_eq!(actions, vec![Action::Key { key: *key }], "context consumption excludes fallback output");
                } else {
                    prop_assert!(actions.is_empty(), "unmapped presses use the empty fallback");
                }
            } else {
                prop_assert!(actions.is_empty(), "releases use the empty fallback");
            }
        }
    }

    #[test]
    fn first_context_with_matching_app_id_wins() {
        let first = tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'tab'").unwrap();
        let second = tehuti_protocol::parse_context(
            "app_id = 'test'\n[buttons.east]\nkey = 'escape'\n[buttons.north]\nkey = 'space'",
        )
        .unwrap();
        let (state, _) = step(
            State::with_contexts(vec![first, second]),
            &Event::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            },
        );
        let (state, actions) = step(
            state,
            &Event::Button {
                name: ButtonName::East,
                state: ButtonState::Pressed,
            },
        );
        assert_eq!(actions, vec![Action::Key { key: KeyName::Tab }]);
        let (_, unmapped_actions) = step(
            state,
            &Event::Button {
                name: ButtonName::North,
                state: ButtonState::Pressed,
            },
        );
        assert!(
            unmapped_actions.is_empty(),
            "later duplicate contexts cannot fill missing mappings"
        );
    }

    #[test]
    fn focus_selects_and_clears_the_active_context() {
        let context = tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'tab'").unwrap();
        let mut state = State::with_contexts(vec![context]);
        for app_id in [Some("test"), Some("other"), Some("test"), None] {
            (state, _) = step(
                state,
                &Event::Focus {
                    app_id: app_id.map(str::to_owned),
                    title: String::new(),
                },
            );
            let (next, actions) = step(
                state,
                &Event::Button {
                    name: ButtonName::East,
                    state: ButtonState::Pressed,
                },
            );
            state = next;
            assert_eq!(
                actions,
                if app_id == Some("test") {
                    vec![Action::Key { key: KeyName::Tab }]
                } else {
                    vec![]
                },
                "only matching focus selects the context"
            );
        }
    }

    #[test]
    fn pressing_a_sends_enter() {
        assert_eq!(
            run(&[A_PRESSED]),
            vec![Action::Key { key: KeyName::Enter }],
            "the global layer maps A to Enter"
        );
    }

    #[test]
    fn releasing_a_sends_nothing() {
        let released = Event::Button {
            name: ButtonName::South,
            state: ButtonState::Released,
        };
        assert_eq!(run(&[released]), vec![], "Enter is a full tap on press");
    }
}
