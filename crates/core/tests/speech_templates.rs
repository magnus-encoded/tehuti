//! Prefix commands run through the same context and core seams as fixed phrases.
#[cfg(test)]
mod speech_templates {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, Event, KeyName, Text, parse_context};

    #[test]
    fn invoke_tail_is_typed_in_its_context_and_fixed_phrases_win() {
        let context = parse_context("app_id = 'agent'\n[buttons]\n[speech]\n'Invoke <name>' = { text = '/{name} ' }\n'Invoke help' = { key = 'enter' }").unwrap();
        assert_eq!(
            context.speech_action("Invoke Review-Code."),
            Some(Action::Text {
                text: Text::new("/Review-Code ".into()).unwrap(),
            })
        );
        assert_eq!(
            context.speech_action("Invoke help."),
            Some(Action::Key {
                key: KeyName::Enter,
                mods: vec![]
            })
        );
        assert_eq!(context.speech_action("Invoke."), None);
        assert_eq!(
            context.speech_action("Invoke name."),
            Some(Action::Text {
                text: Text::new("/name ".into()).unwrap(),
            })
        );
        let (_, actions) = step(
            State::with_contexts(vec![context]),
            &Event::Speech {
                text: Text::new("Invoke review".into()).unwrap(),
            },
        );
        assert!(matches!(actions.as_slice(), [Action::Notice { .. }]));
    }
    #[test]
    fn type_converts_symbols_letters_digits_and_named_keys_without_partial_input() {
        let (_, actions) = step(
            State::new(),
            &Event::Speech {
                text: Text::new("Type slash a one Enter Backspace.".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![
                Action::Text {
                    text: Text::new("/".into()).unwrap()
                },
                Action::Key {
                    key: KeyName::A,
                    mods: vec![]
                },
                Action::Key {
                    key: KeyName::Digit1,
                    mods: vec![]
                },
                Action::Key {
                    key: KeyName::Enter,
                    mods: vec![]
                },
                Action::Key {
                    key: KeyName::Backspace,
                    mods: vec![]
                },
            ]
        );
        let (_, rejected_actions) = step(
            State::new(),
            &Event::Speech {
                text: Text::new("Type a unknown-key.".into()).unwrap(),
            },
        );
        assert!(matches!(rejected_actions.as_slice(), [Action::Notice { .. }]));
    }
    #[test]
    fn templates_require_a_text_slot_and_a_nonempty_prefix() {
        for entry in [
            "'Invoke <name>' = { key = 'enter' }",
            "'Invoke <name>' = { text = '/fixed' }",
            "'<name>' = { text = '/{name}' }",
            "'Work on <project>' = { text = 'cd fixed' }",
        ] {
            assert!(parse_context(&format!("app_id = 'agent'\n[buttons]\n[speech]\n{entry}")).is_err());
        }
    }

    #[test]
    fn fixed_type_mapping_wins_and_multiword_keys_are_spoken() {
        let context =
            parse_context("app_id = '*'\n[buttons]\n[speech]\n'Type slash' = { text = 'custom' }").unwrap();
        let (_, actions) = step(
            State::with_contexts(vec![context]),
            &Event::Speech {
                text: Text::new("Type slash.".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Text {
                text: Text::new("custom".into()).unwrap()
            }]
        );
        for (heard, expected) in [
            ("Type open bracket.", "["),
            ("Type double quote.", "\""),
            ("Type dollar sign.", "$"),
            ("Type question mark.", "?"),
        ] {
            let (_, symbol_actions) = step(
                State::new(),
                &Event::Speech {
                    text: Text::new(heard.into()).unwrap(),
                },
            );
            assert_eq!(
                symbol_actions,
                vec![Action::Text {
                    text: Text::new(expected.into()).unwrap()
                }]
            );
        }
        let (_, named_key_actions) = step(
            State::new(),
            &Event::Speech {
                text: Text::new("Type page up space nine.".into()).unwrap(),
            },
        );
        assert_eq!(
            named_key_actions,
            vec![
                Action::Key {
                    key: KeyName::PageUp,
                    mods: vec![]
                },
                Action::Key {
                    key: KeyName::Space,
                    mods: vec![]
                },
                Action::Key {
                    key: KeyName::Digit9,
                    mods: vec![]
                },
            ]
        );
    }
    fn configured_for_spoken_keys(app_id: &str, phrase: &str, global: bool, focused: bool) -> State {
        let context = parse_context(&format!(
            "app_id = '{}'\n[buttons]\nsouth = {{ text = 'configured' }}\n[speech]\n'{phrase}' = {{ text = 'mapped' }}",
            if global { "*" } else { app_id },
        )).unwrap();
        step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: focused.then(|| app_id.to_owned()),
                title: String::new(),
            },
        )
        .0
    }

    proptest! {
        #[test]
        fn spoken_key_sequences_compose_in_every_context(
            app_id in "[a-z]{1,16}",
            phrase in "fixed[a-z]{1,16}",
            global in any::<bool>(),
            focused in any::<bool>(),
            names in prop::collection::vec(prop::sample::select(vec![
                "a", "z", "zero", "nine", "slash", "double quote", "left bracket",
                "page up", "enter", "space", "backspace",
            ]), 2..20),
        ) {
            let state = configured_for_spoken_keys(&app_id, &phrase, global, focused);
            let expected: Vec<_> = names.iter().flat_map(|name| step(state.clone(), &Event::Speech {
                text: Text::new(format!("Type {name}.")).unwrap(),
            }).1).collect();
            let (_, actual) = step(state.clone(), &Event::Speech {
                text: Text::new(format!("Type {}.", names.join(" "))).unwrap(),
            });
            prop_assert_eq!(actual, expected);
            let (_, rejected) = step(state, &Event::Speech {
                text: Text::new(format!("Type {} unknown-key.", names.join(" "))).unwrap(),
            });
            prop_assert!(matches!(rejected.as_slice(), [Action::Notice { .. }]), "invalid sequence emits only a notice");
        }
    }
}
