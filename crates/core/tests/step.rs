//! Properties of the core step over generated event series.

#[cfg(test)]
mod step {
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

    proptest! {
        #[test]
        fn routing_stops_at_the_first_consuming_layer(events in prop::collection::vec(prop_oneof![any::<Event>(), Just(Event::Focus { app_id: "test".into(), title: String::new() })], 0..200)) {
            let context = tehuti_protocol::parse_context("app_id = 'test'\n[buttons.south]\nkey = 'escape'\n[buttons.east]\nkey = 'tab'").unwrap();
            let mut state = State::with_contexts(vec![context]);
            let mut focused = String::new();
            for event in events {
                let expected = match &event {
                    Event::Focus { app_id, .. } => { focused.clone_from(app_id); vec![] }
                    Event::Button { name: ButtonName::South, state: ButtonState::Pressed } => vec![Action::Key { key: KeyName::Enter }],
                    Event::Button { name: ButtonName::East, state: ButtonState::Pressed } if focused == "test" => vec![Action::Key { key: KeyName::Tab }],
                    Event::Button { .. } => vec![],
                };
                let (next, actions) = step(state, &event);
                state = next;
                prop_assert_eq!(actions, expected);
            }
        }
    }

    #[test]
    fn focus_selects_and_clears_the_active_context() {
        let context = tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'tab'").unwrap();
        let mut state = State::with_contexts(vec![context]);
        for app_id in ["test", "other", "test", ""] {
            (state, _) = step(
                state,
                &Event::Focus {
                    app_id: app_id.into(),
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
                if app_id == "test" {
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
