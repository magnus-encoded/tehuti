#[cfg(test)]
mod triggers {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, parse_context};

    fn focused(buttons: &str) -> State {
        let context = parse_context(&format!("app_id = 'terminal'\n[buttons]\n{buttons}")).unwrap();
        step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("terminal".into()),
                title: String::new(),
            },
        )
        .0
    }

    fn button(name: ButtonName, pressed: bool) -> Event {
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        Event::Button { name, state }
    }

    #[test]
    fn a_context_binds_both_triggers() {
        let state = focused("left_trigger = { key = 'page_up' }\nright_trigger = { key = 'page_down' }");
        let (state, left) = step(state, &button(ButtonName::LeftTrigger, true));
        let (state, _) = step(state, &button(ButtonName::LeftTrigger, false));
        let (_, right) = step(state, &button(ButtonName::RightTrigger, true));
        let key = |key| {
            vec![Action::Key {
                key,
                mods: Vec::new(),
            }]
        };
        assert_eq!(left, key(KeyName::PageUp));
        assert_eq!(right, key(KeyName::PageDown));
    }

    proptest! {
        /// A bound trigger dispatches presses and releases exactly as a bound bumper does.
        #[test]
        fn triggers_dispatch_like_bumpers(
            presses in prop::collection::vec((any::<bool>(), any::<bool>()), 0..24),
        ) {
            let mut triggers = focused("left_trigger = { key = 'tab' }\nright_trigger = { key = 'enter' }");
            let mut bumpers = focused("left_bumper = { key = 'tab' }\nright_bumper = { key = 'enter' }");
            for (left, pressed) in presses {
                let (trigger, bumper) = if left {
                    (ButtonName::LeftTrigger, ButtonName::LeftBumper)
                } else {
                    (ButtonName::RightTrigger, ButtonName::RightBumper)
                };
                let (stepped, from_trigger) = step(triggers, &button(trigger, pressed));
                triggers = stepped;
                let (stepped_bumpers, from_bumper) = step(bumpers, &button(bumper, pressed));
                bumpers = stepped_bumpers;
                prop_assert_eq!(from_trigger, from_bumper);
            }
        }
    }
}
