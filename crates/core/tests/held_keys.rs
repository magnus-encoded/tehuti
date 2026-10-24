//! Held-key mappings: a button keeps its key down until the button's release.

#[cfg(test)]
mod held_keys {
    use core::fmt::Write as _;

    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, parse_context};

    fn focused(source: &str) -> State {
        let context = parse_context(source).unwrap();
        let app_id = context.app_id().to_owned();
        step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some(app_id),
                title: String::new(),
            },
        )
        .0
    }

    fn button(state: State, name: ButtonName, button: ButtonState) -> (State, Vec<Action>) {
        step(state, &Event::Button { name, state: button })
    }

    #[test]
    fn a_held_key_goes_down_on_press_and_up_on_release() {
        let state = focused("app_id = 'emulator'\n[buttons]\nwest = { key = 'backspace', hold = true }");
        let (state, pressed) = button(state, ButtonName::West, ButtonState::Pressed);
        assert_eq!(
            pressed,
            vec![Action::KeyDown {
                key: KeyName::Backspace
            }]
        );
        let (_, released) = button(state, ButtonName::West, ButtonState::Released);
        assert_eq!(
            released,
            vec![Action::KeyUp {
                key: KeyName::Backspace
            }]
        );
    }

    #[test]
    fn a_plain_key_mapping_still_taps_on_press() {
        let state = focused(
            "app_id = 'emulator'\n[buttons]\nwest = { key = 'backspace' }\neast = { key = 'z', hold = false }",
        );
        for name in [ButtonName::West, ButtonName::East] {
            let (next, pressed) = button(state.clone(), name, ButtonState::Pressed);
            assert!(matches!(pressed.as_slice(), [Action::Key { mods, .. }] if mods.is_empty()));
            let (_, released) = button(next, name, ButtonState::Released);
            assert_eq!(released, Vec::new());
        }
    }

    #[test]
    fn a_held_south_takes_the_place_of_the_global_enter() {
        let state = focused("app_id = 'emulator'\n[buttons]\nsouth = { key = 'x', hold = true }");
        let (state, pressed) = button(state, ButtonName::South, ButtonState::Pressed);
        assert_eq!(pressed, vec![Action::KeyDown { key: KeyName::X }]);
        let (_, released) = button(state, ButtonName::South, ButtonState::Released);
        assert_eq!(released, vec![Action::KeyUp { key: KeyName::X }]);
        let tapped = focused("app_id = 'emulator'\n[buttons]\nsouth = { key = 'x' }");
        let (_, tapped_south) = button(tapped, ButtonName::South, ButtonState::Pressed);
        assert_eq!(
            tapped_south,
            vec![Action::Key {
                key: KeyName::Enter,
                mods: Vec::new()
            }],
            "a plain South mapping keeps the global Enter"
        );
    }

    #[test]
    fn two_buttons_holding_one_key_press_it_once_and_release_it_with_the_last() {
        let state = focused(
            "app_id = 'emulator'\n[buttons]\nnorth = { key = 'enter', hold = true }\nstart = { key = 'enter', hold = true }",
        );
        let (state, first) = button(state, ButtonName::North, ButtonState::Pressed);
        assert_eq!(first, vec![Action::KeyDown { key: KeyName::Enter }]);
        let (state, second) = button(state, ButtonName::Start, ButtonState::Pressed);
        assert_eq!(second, Vec::new());
        let (state, early) = button(state, ButtonName::North, ButtonState::Released);
        assert_eq!(early, Vec::new(), "Start still holds Enter");
        let (_, last) = button(state, ButtonName::Start, ButtonState::Released);
        assert_eq!(last, vec![Action::KeyUp { key: KeyName::Enter }]);
    }

    #[test]
    fn a_held_key_lifts_on_its_release_after_focus_moves_to_another_context() {
        let emulator =
            parse_context("app_id = 'emulator'\n[buttons]\ndpad_up = { key = 'up', hold = true }").unwrap();
        let other = parse_context("app_id = 'other'\n[buttons]\ndpad_up = { key = 'w' }").unwrap();
        let state = step(
            State::with_contexts(vec![emulator, other]),
            &Event::Focus {
                app_id: Some("emulator".into()),
                title: String::new(),
            },
        )
        .0;
        let (state, _) = button(state, ButtonName::DpadUp, ButtonState::Pressed);
        let (state, _) = step(
            state,
            &Event::Focus {
                app_id: Some("other".into()),
                title: String::new(),
            },
        );
        let (_, released) = button(state, ButtonName::DpadUp, ButtonState::Released);
        assert_eq!(released, vec![Action::KeyUp { key: KeyName::Up }]);
    }

    #[test]
    fn plugins_cannot_hold_or_release_keys() {
        for action in [
            Action::KeyDown { key: KeyName::X },
            Action::KeyUp { key: KeyName::X },
        ] {
            assert!(
                tehuti_core::plugin::validate_actions(vec![action], &[tehuti_protocol::Permission::Key])
                    .is_err()
            );
        }
    }

    fn arbitrary_button() -> impl Strategy<Value = ButtonName> {
        prop::sample::select(vec![
            ButtonName::South,
            ButtonName::East,
            ButtonName::North,
            ButtonName::West,
            ButtonName::DpadUp,
            ButtonName::DpadDown,
            ButtonName::Guide,
            ButtonName::Start,
        ])
    }

    proptest! {
        #[test]
        fn a_key_is_down_only_while_a_button_holding_it_is_pressed(
            holds in prop::collection::hash_map(arbitrary_button(), prop::sample::select(vec![KeyName::X, KeyName::Z, KeyName::Up]), 0..6),
            events in prop::collection::vec((arbitrary_button(), any::<bool>()), 0..80),
        ) {
            let mut source = String::from("app_id = 'emulator'\n[buttons]\n");
            for (name, key) in &holds {
                let name = toml::Value::try_from(name).unwrap();
                writeln!(source, "{} = {{ key = '{key:?}', hold = true }}", name.as_str().unwrap()).unwrap();
            }
            let mut state = focused(&source);
            let mut pressed: Vec<ButtonName> = Vec::new();
            let mut down: Vec<KeyName> = Vec::new();
            for (name, press) in events {
                let (next, actions) = button(state, name, if press { ButtonState::Pressed } else { ButtonState::Released });
                state = next;
                pressed.retain(|held| *held != name);
                if press {
                    pressed.push(name);
                }
                for action in actions {
                    if let Action::KeyDown { key } = action {
                        prop_assert!(!down.contains(&key), "a held key goes down once");
                        down.push(key);
                    } else if let Action::KeyUp { key } = action {
                        prop_assert!(down.contains(&key), "only a held key goes up");
                        down.retain(|held| *held != key);
                    }
                }
                for key in &down {
                    prop_assert!(pressed.iter().any(|holder| holds.get(holder) == Some(key)), "{key:?} is down with no button holding it");
                }
            }
        }
    }
}
