//! Properties of the core step over generated event series.

#[cfg(test)]
mod step {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName};

    fn button_event() -> impl Strategy<Value = Event> {
        (
            proptest::sample::select(ButtonName::ALL.to_vec()),
            prop_oneof![Just(ButtonState::Pressed), Just(ButtonState::Released)],
        )
            .prop_map(|(name, state)| Event::Button { name, state })
    }

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
        fn step_returns_for_any_event_series(events in prop::collection::vec(button_event(), 0..200)) {
            let _actions = run(&events);
        }

        #[test]
        fn each_a_press_sends_one_enter_and_nothing_else_reaches_the_client(
            events in prop::collection::vec(button_event(), 0..200),
        ) {
            let a_presses = events.iter().filter(|event| **event == A_PRESSED).count();
            prop_assert_eq!(run(&events), vec![Action::Key { key: KeyName::Enter }; a_presses]);
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
