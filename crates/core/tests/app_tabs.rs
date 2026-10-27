#[cfg(test)]
mod app_tabs {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Modifier, Text, parse_context};

    fn focused(contexts: Vec<tehuti_protocol::Context>) -> State {
        step(
            State::with_contexts(contexts),
            &Event::Focus {
                app_id: Some("org.example.Editor".into()),
                title: String::new(),
            },
        )
        .0
    }

    fn speech(text: &str) -> Event {
        Event::Speech {
            text: Text::new(text.into()).unwrap(),
        }
    }

    fn button(name: ButtonName, state: ButtonState) -> Event {
        Event::Button { name, state }
    }

    #[test]
    fn global_app_level_shoulders_send_tab_taps_and_releases_send_nothing() {
        let mut state = focused(vec![
            parse_context(include_str!("../../../examples/global/context.toml")).unwrap(),
        ]);
        for (name, mods) in [
            (ButtonName::LeftBumper, vec![Modifier::Shift]),
            (ButtonName::RightBumper, vec![]),
        ] {
            let phrase = if name == ButtonName::LeftBumper {
                "tab left"
            } else {
                "tab right"
            };
            assert_eq!(
                step(state.clone(), &speech(phrase)).1,
                vec![Action::Key {
                    key: KeyName::Tab,
                    mods: mods.clone()
                }]
            );
            let (next, actions) = step(state, &button(name, ButtonState::Pressed));
            assert_eq!(
                actions,
                vec![Action::Key {
                    key: KeyName::Tab,
                    mods
                }]
            );
            let (next, released) = step(next, &button(name, ButtonState::Released));
            assert_eq!(released, Vec::<Action>::new());
            state = next;
        }
    }
    #[test]
    fn spoken_tabs_use_the_app_level_button_mapping_and_context_override() {
        for (mapping, expected) in [
            (
                "left_bumper = { key = 'tab', mods = ['shift'] }\nright_bumper = { key = 'tab' }",
                vec![
                    Action::Key {
                        key: KeyName::Tab,
                        mods: vec![Modifier::Shift],
                    },
                    Action::Key {
                        key: KeyName::Tab,
                        mods: vec![],
                    },
                ],
            ),
            (
                "left_bumper = { key = 'page_up', mods = ['ctrl'] }\nright_bumper = { key = 'page_down', mods = ['ctrl'] }",
                vec![
                    Action::Key {
                        key: KeyName::PageUp,
                        mods: vec![Modifier::Ctrl],
                    },
                    Action::Key {
                        key: KeyName::PageDown,
                        mods: vec![Modifier::Ctrl],
                    },
                ],
            ),
        ] {
            let global = parse_context(include_str!("../../../examples/global/context.toml")).unwrap();
            let app = parse_context(&format!("app_id = 'org.example.Editor'\n[buttons]\n{mapping}")).unwrap();
            let state = focused(vec![global, app]);
            let spoken: Vec<_> = ["TAB LEFT!", "tab right"]
                .into_iter()
                .flat_map(|phrase| step(state.clone(), &speech(phrase)).1)
                .collect();
            let pressed: Vec<_> = [ButtonName::LeftBumper, ButtonName::RightBumper]
                .into_iter()
                .flat_map(|name| step(state.clone(), &button(name, ButtonState::Pressed)).1)
                .collect();
            assert_eq!(pressed, expected);
            assert_eq!(spoken, expected);
        }
    }

    #[test]
    fn passthrough_keeps_shoulders_with_the_application_and_spoken_tabs_send_no_keys() {
        for app in [None, Some(false), Some(true)] {
            let global = parse_context("app_id = '*'\n[buttons]\nleft_bumper = { key = 'tab' }\nright_bumper = { key = 'tab' }\n[pad]\npassthrough = true").unwrap();
            let mut contexts = vec![global];
            if let Some(passthrough) = app {
                contexts.push(
                    parse_context(&format!(
                        "app_id = 'org.example.Editor'\nbuttons = {{}}\n[pad]\npassthrough = {passthrough}"
                    ))
                    .unwrap(),
                );
            }
            let state = focused(contexts);
            let passthrough = app.unwrap_or(true);
            assert_eq!(state.passthrough(), passthrough);
            for (name, phrase) in [
                (ButtonName::LeftBumper, "tab left"),
                (ButtonName::RightBumper, "tab right"),
            ] {
                for event in [button(name, ButtonState::Pressed), speech(phrase)] {
                    let expected = if passthrough {
                        vec![]
                    } else {
                        vec![Action::Key {
                            key: KeyName::Tab,
                            mods: vec![],
                        }]
                    };
                    assert_eq!(step(state.clone(), &event).1, expected);
                }
            }
        }
    }

    #[test]
    fn left_and_right_keep_moving_column_focus_at_both_levels() {
        let mut state = focused(vec![
            parse_context(include_str!("../../../examples/global/context.toml")).unwrap(),
        ]);
        for _ in 0..2 {
            assert_eq!(
                step(state.clone(), &speech("left")).1,
                vec![Action::Layout {
                    action: tehuti_protocol::LayoutAction::FocusColumnLeft
                }]
            );
            assert_eq!(
                step(state.clone(), &speech("right")).1,
                vec![Action::Layout {
                    action: tehuti_protocol::LayoutAction::FocusColumnRight
                }]
            );
            state = step(state, &button(ButtonName::Guide, ButtonState::Pressed)).0;
            state = step(state, &button(ButtonName::Guide, ButtonState::Released)).0;
        }
    }

    proptest! {
        #[test]
        fn spoken_tabs_follow_generated_global_and_app_mappings(
            app_id in "org[.]example[.][A-Za-z]{1,12}",
            global_key in prop::sample::select(vec!["tab", "page_up", "page_down"]),
            app_key in prop::option::of(prop::sample::select(vec!["tab", "page_up", "page_down"])),
            shift in any::<bool>(),
        ) {
            let mods = if shift { "['shift']" } else { "['ctrl']" };
            let global = parse_context(&format!("app_id = '*'\n[buttons]\nleft_bumper = {{ key = '{global_key}', mods = {mods} }}\nright_bumper = {{ key = '{global_key}' }}")).unwrap();
            let mut contexts = vec![global];
            if let Some(key) = app_key {
                contexts.push(parse_context(&format!("app_id = '{app_id}'\n[buttons]\nleft_bumper = {{ key = '{key}' }}\nright_bumper = {{ key = '{key}', mods = {mods} }}")).unwrap());
            }
            let state = step(State::with_contexts(contexts), &Event::Focus { app_id: Some(app_id), title: String::new() }).0;
            for (name, phrase) in [(ButtonName::LeftBumper, "tab left"), (ButtonName::RightBumper, "tab right")] {
                prop_assert_eq!(step(state.clone(), &speech(phrase)).1, step(state.clone(), &button(name, ButtonState::Pressed)).1);
            }
        }
    }
}
