#[cfg(test)]
mod speech_routing {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, Event, KeyName, Text, parse_context};

    #[test]
    fn focused_phrase_runs_its_mapping_and_unmatched_phrase_shows_notice() {
        let context =
            parse_context("app_id = 'terminal'\n[buttons]\n[speech]\nconfirm = { key = 'enter' }").unwrap();
        let (state, _) = step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("terminal".into()),
                title: String::new(),
            },
        );
        let (state, actions) = step(
            state,
            &Event::Speech {
                text: Text::new("confirm".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Key {
                key: KeyName::Enter,
                mods: Vec::new()
            }],
            "focused phrase executes its context action"
        );
        let (_, unmatched_actions) = step(
            state,
            &Event::Speech {
                text: Text::new("unknown".into()).unwrap(),
            },
        );
        assert!(
            matches!(unmatched_actions.as_slice(), [Action::Notice { .. }]),
            "unmatched speech emits a user notice"
        );
    }

    #[test]
    fn whisper_casing_whitespace_and_trailing_punctuation_still_match() {
        let context =
            parse_context("app_id = 'terminal'\n[buttons]\n[speech]\nhello = { key = 'enter' }").unwrap();
        for heard in ["Hello.", "  HELLO!  ", "hello?", "hello"] {
            assert_eq!(
                context.speech_phrase_action(heard),
                Some(Action::Key {
                    key: KeyName::Enter,
                    mods: Vec::new()
                }),
                "{heard:?} matches the mapped phrase"
            );
        }
        assert_eq!(
            context.speech_phrase_action("hello there"),
            None,
            "other words still differ"
        );
    }
    #[test]
    fn focused_and_global_phrases_precede_unmatched_typing() {
        for focused_type in [false, true] {
            for global_type in [false, true] {
                let context = |app, enabled, mappings| {
                    parse_context(&format!(
                        "app_id = '{app}'\n{}\n[buttons]\n[speech]\n{mappings}",
                        if enabled { "unmatched_speech = 'type'" } else { "" }
                    ))
                    .unwrap()
                };
                let state = step(
                    State::with_contexts(vec![
                        context("terminal", focused_type, "confirm = { key = 'enter' }"),
                        context(
                            "*",
                            global_type,
                            "confirm = { key = 'space' }\nglobal = { key = 'space' }",
                        ),
                    ]),
                    &Event::Focus {
                        app_id: Some("terminal".into()),
                        title: String::new(),
                    },
                )
                .0;
                for (phrase, expected) in [
                    (
                        "Confirm!",
                        Action::Key {
                            key: KeyName::Enter,
                            mods: vec![],
                        },
                    ),
                    (
                        "Global.",
                        Action::Key {
                            key: KeyName::Space,
                            mods: vec![],
                        },
                    ),
                    (
                        "unknown",
                        if focused_type || global_type {
                            Action::Text {
                                text: Text::new("unknown ".into()).unwrap(),
                            }
                        } else {
                            Action::Notice {
                                text: Text::new("NO SPEECH MAPPING".into()).unwrap(),
                            }
                        },
                    ),
                ] {
                    let (_, actions) = step(
                        state.clone(),
                        &Event::Speech {
                            text: Text::new(phrase.into()).unwrap(),
                        },
                    );
                    assert_eq!(actions, vec![expected]);
                }
            }
        }
    }

    fn focused_state(option: &str) -> State {
        let context = parse_context(&format!(
            "app_id = 'terminal'\n{option}\n[buttons]\n[speech]\nconfirm = {{ key = 'enter' }}"
        ))
        .unwrap();
        step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("terminal".into()),
                title: String::new(),
            },
        )
        .0
    }

    #[test]
    fn mapped_phrase_wins_over_dictation() {
        let (_, actions) = step(
            focused_state("unmatched_speech = 'type'"),
            &Event::Speech {
                text: Text::new("Confirm!".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Key {
                key: KeyName::Enter,
                mods: Vec::new()
            }]
        );
    }

    #[test]
    fn unmatched_dictation_keeps_transcript_and_adds_one_space() {
        let (_, actions) = step(
            focused_state("unmatched_speech = 'type'"),
            &Event::Speech {
                text: Text::new("Write this sentence.".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Text {
                text: Text::new("Write this sentence. ".into()).unwrap()
            }]
        );
    }

    #[test]
    fn empty_transcripts_are_ignored_with_or_without_dictation() {
        for option in ["", "unmatched_speech = 'type'"] {
            for text in ["", "  "] {
                let (_, actions) = step(
                    focused_state(option),
                    &Event::Speech {
                        text: Text::new(text.into()).unwrap(),
                    },
                );
                assert_eq!(actions, Vec::<Action>::new());
            }
        }
    }

    #[test]
    fn dictation_option_applies_only_to_its_focused_context() {
        let (state, _) = step(
            focused_state("unmatched_speech = 'type'"),
            &Event::Focus {
                app_id: Some("other".into()),
                title: String::new(),
            },
        );
        let (_, actions) = step(
            state,
            &Event::Speech {
                text: Text::new("hello".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Notice {
                text: Text::new("NO SPEECH MAPPING".into()).unwrap()
            }]
        );
    }

    #[test]
    fn invalid_dictation_modes_are_rejected() {
        assert!(parse_context("app_id = 'terminal'\nunmatched_speech = 'invalid'\n[buttons]").is_err());
    }

    proptest! {
        #[test]
        fn dictation_routes_only_in_its_context(
            app_id in "[a-z]{1,20}",
            phrase in "[a-z]{1,20}",
            utterance in "[A-Z][a-z ]{1,80}",
            enabled in any::<bool>(),
            focused in any::<bool>(),
        ) {
            let option = if enabled { "unmatched_speech = 'type'" } else { "" };
            let context = parse_context(&format!(
                "app_id = '{app_id}'\n{option}\n[buttons]\n[speech]\n'{phrase}' = {{ key = 'enter' }}"
            )).unwrap();
            let (state, _) = step(
                State::with_contexts(vec![context]),
                &Event::Focus {
                    app_id: focused.then_some(app_id),
                    title: String::new(),
                },
            );
            let (_, actions) = step(state, &Event::Speech { text: Text::new(utterance.clone()).unwrap() });
            if focused && utterance.trim().eq_ignore_ascii_case(&phrase) {
                prop_assert_eq!(actions, vec![Action::Key { key: KeyName::Enter, mods: Vec::new() }]);
            } else if focused && enabled {
                prop_assert_eq!(actions, vec![Action::Text { text: Text::new(format!("{utterance} ")).unwrap() }]);
            } else {
                prop_assert_eq!(actions, vec![Action::Notice { text: Text::new("NO SPEECH MAPPING".into()).unwrap() }]);
            }
        }
    }
}
