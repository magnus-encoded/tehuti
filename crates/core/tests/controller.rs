//! Controller laws over arbitrary session and focus interleavings.

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use proptest::prelude::*;
    use tehuti_core::controller::{self, Input};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName};

    fn interleavings() -> impl Strategy<Value = Vec<(u8, ButtonName, ButtonState)>> {
        prop::collection::vec((0_u8..7, any::<ButtonName>(), any::<ButtonState>()), 0..200)
    }

    fn input(kind: u8, name: ButtonName, state: ButtonState) -> Input {
        match kind {
            0 | 1 => Input::Event(Event::Button { name, state }),
            2 => Input::Disconnected,
            3 => Input::Reconnected,
            _ => Input::Event(Event::Focus {
                app_id: match kind {
                    4 => Some("test".into()),
                    5 => Some("other".into()),
                    _ => None,
                },
                title: String::new(),
            }),
        }
    }

    fn mappings() -> impl Strategy<Value = std::collections::HashMap<ButtonName, KeyName>> {
        prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=16)
    }

    fn context(mappings: &std::collections::HashMap<ButtonName, KeyName>) -> tehuti_core::State {
        let mut config = String::from("app_id = 'test'\n[buttons]\n");
        for (button, key) in mappings {
            let button_value = toml::Value::try_from(button).expect("button serializes");
            let button_name = button_value.as_str().expect("button is a name");
            let key_value = toml::Value::try_from(key).expect("key serializes");
            write!(config, "[buttons.{button_name}]\nkey = {key_value}\n").expect("string accepts mappings");
        }
        tehuti_core::State::with_contexts(vec![
            tehuti_protocol::parse_context(&config).expect("generated context is valid"),
        ])
    }

    fn forward(core: &mut tehuti_core::State, events: &[Event]) -> Vec<(KeyName, ButtonState)> {
        let mut output = Vec::new();
        for event in events {
            let (next, actions) = tehuti_core::step(core::mem::take(core), event);
            *core = next;
            for action in actions {
                if let Action::Key { key } = action {
                    output.extend(controller::key_tap().map(|state| (key, state)));
                }
            }
        }
        output
    }

    proptest! {
        #[test]
        fn every_forwarded_press_gets_exactly_one_release(sequence in interleavings()) {
            let mut session = controller::State::default();
            let mut pressed = std::collections::HashSet::new();
            for (kind, name, state) in sequence.into_iter().chain(core::iter::once((2, ButtonName::South, ButtonState::Released))) {
                let (next, events) = controller::step(session, input(kind, name, state));
                session = next;
                for event in events {
                    if let Event::Button { name: output_name, state: output_state } = event {
                        match output_state {
                            ButtonState::Pressed => prop_assert!(pressed.insert(output_name), "duplicate press"),
                            ButtonState::Released => prop_assert!(pressed.remove(&output_name), "unpaired release"),
                        }
                    }
                }
            }
            prop_assert!(pressed.is_empty(), "a forwarded press lost its release");
        }

        #[test]
        fn disconnect_leaves_no_button_held(sequence in interleavings()) {
            let mut session = controller::State::default();
            let mut pressed = std::collections::HashSet::new();
            for (kind, name, state) in sequence.into_iter().chain(core::iter::once((2, ButtonName::South, ButtonState::Released))) {
                let (next, events) = controller::step(session, input(kind, name, state));
                session = next;
                for event in events {
                    if let Event::Button { name: output_name, state: output_state } = event {
                        if output_state == ButtonState::Pressed { let _inserted = pressed.insert(output_name); }
                        else { let _removed = pressed.remove(&output_name); }
                    }
                }
                if kind == 2 { prop_assert!(pressed.is_empty()); }
            }
        }

        #[test]
        fn fresh_presses_pair_after_each_disconnect(button in any::<ButtonName>(), sessions in 1_usize..20) {
            let mut session = controller::State::default();
            for _ in 0..sessions {
                let (next, _) = controller::step(session, Input::Disconnected);
                let (next, _) = controller::step(next, Input::Reconnected);
                let release = Event::Button { name: button, state: ButtonState::Released };
                let (next, stale) = controller::step(next, Input::Event(release.clone()));
                prop_assert!(stale.is_empty());
                let press = Event::Button { name: button, state: ButtonState::Pressed };
                let (next, pressed) = controller::step(next, Input::Event(press.clone()));
                prop_assert_eq!(pressed, vec![press]);
                let (next, released) = controller::step(next, Input::Event(release.clone()));
                prop_assert_eq!(released, vec![release]);
                session = next;
            }
        }

        #[test]
        fn reconnect_cannot_release_a_previous_session_press(sequence in interleavings(), configuration in mappings(), button in any::<ButtonName>()) {
            let mut session = controller::State::default();
            let mut core = context(&configuration);
            for (kind, name, state) in sequence {
                let (next, events) = controller::step(session, input(kind, name, state));
                session = next;
                let _output = forward(&mut core, &events);
                if kind == 3 { prop_assert!(events.is_empty()); }
            }
            let (session, _) = controller::step(session, Input::Reconnected);
            let (session, pressed_events) = controller::step(session, Input::Event(Event::Button { name: button, state: ButtonState::Pressed }));
            let _pressed_output = forward(&mut core, &pressed_events);
            let (session, disconnected_events) = controller::step(session, Input::Disconnected);
            let _output = forward(&mut core, &disconnected_events);
            let (session, reconnected_events) = controller::step(session, Input::Reconnected);
            prop_assert!(reconnected_events.is_empty());
            let (_, released_events) = controller::step(session, Input::Event(Event::Button { name: button, state: ButtonState::Released }));
            prop_assert!(released_events.is_empty(), "stale release crossed the disconnect");
        }
    }
    #[test]
    fn disconnect_cancels_a_held_quit_chord() {
        let session = controller::State::default();
        let mut core = tehuti_core::State::new();
        let (session, press) = controller::step(
            session,
            Input::Event(Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Pressed,
            }),
        );
        let _press_output = forward(&mut core, &press);
        let (session, start_press) = controller::step(
            session,
            Input::Event(Event::Button {
                name: ButtonName::Start,
                state: ButtonState::Pressed,
            }),
        );
        let _start_output = forward(&mut core, &start_press);
        let (session, disconnect) = controller::step(session, Input::Disconnected);
        core.controller_disconnected();
        let _disconnect_output = forward(&mut core, &disconnect);
        let (session, _) = controller::step(session, Input::Reconnected);
        let (_, reconnect_press) = controller::step(
            session,
            Input::Event(Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Pressed,
            }),
        );
        let _reconnect_output = forward(&mut core, &reconnect_press);
        let (_, actions) = tehuti_core::step(
            core,
            &Event::Tick {
                elapsed: core::time::Duration::from_secs(4),
            },
        );
        assert!(!actions.contains(&Action::Quit));
    }
}
