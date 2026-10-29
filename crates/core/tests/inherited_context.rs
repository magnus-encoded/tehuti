#[cfg(test)]
mod inherited_context {
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, Event, Text, parse_context};

    #[test]
    fn a_contextless_window_routes_through_its_parent_and_then_global_phrases() {
        let parent =
            parse_context("app_id = 'launcher'\n[buttons]\n[speech.Play]\nnotify = 'playing'").unwrap();
        let global = parse_context("app_id = '*'\n[buttons]\n[speech.Help]\nnotify = 'help'").unwrap();
        let mut state = State::with_contexts(vec![parent, global]);
        state.set_context_ancestors(Some("player".into()), vec!["launcher".into()]);
        let (state, _) = step(
            state,
            &Event::Focus {
                app_id: Some("player".into()),
                title: "Movie".into(),
            },
        );
        let (_, actions) = step(
            state.clone(),
            &Event::Speech {
                text: Text::new("Play".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Notice {
                text: Text::new("playing".into()).unwrap()
            }]
        );
        let (_, global_actions) = step(
            state,
            &Event::Speech {
                text: Text::new("Help.".into()).unwrap(),
            },
        );
        assert_eq!(
            global_actions,
            vec![Action::Notice {
                text: Text::new("help".into()).unwrap()
            }]
        );
    }

    #[test]
    fn close_works_without_any_context() {
        let (_, actions) = step(
            State::new(),
            &Event::Speech {
                text: Text::new("Close.".into()).unwrap(),
            },
        );
        assert_eq!(actions, vec![Action::Close]);
    }

    #[test]
    fn own_context_wins_and_an_unmapped_button_uses_global_not_parent() {
        use tehuti_protocol::{ButtonName, ButtonState, KeyName};
        let own = parse_context("app_id = 'player'\n[buttons]\nwest = { key = 'space' }").unwrap();
        let parent = parse_context(
            "app_id = 'launcher'\n[buttons]\nwest = { key = 'escape' }\neast = { key = 'tab' }",
        )
        .unwrap();
        let global = parse_context("app_id = '*'\n[buttons]\neast = { key = 'enter' }").unwrap();
        let mut state = State::with_contexts(vec![own, parent, global]);
        state.set_context_ancestors(Some("player".into()), vec!["launcher".into()]);
        let (state, _) = step(
            state,
            &Event::Focus {
                app_id: Some("player".into()),
                title: String::new(),
            },
        );
        for (name, key) in [
            (ButtonName::West, KeyName::Space),
            (ButtonName::East, KeyName::Enter),
        ] {
            let (_, actions) = step(
                state.clone(),
                &Event::Button {
                    name,
                    state: ButtonState::Pressed,
                },
            );
            assert_eq!(
                actions,
                vec![Action::Key {
                    key,
                    mods: Vec::new()
                }]
            );
        }
    }

    #[test]
    fn nearest_matching_ancestor_supplies_buttons_and_reload_reselects_context() {
        use tehuti_protocol::{ButtonName, ButtonState, KeyName};
        let parent = parse_context("app_id = 'launcher'\n[buttons]\nwest = { key = 'escape' }").unwrap();
        let root = parse_context("app_id = 'terminal'\n[buttons]\nwest = { key = 'tab' }").unwrap();
        let mut state = State::with_contexts(vec![root, parent]);
        state.set_context_ancestors(None, vec!["unknown".into(), "launcher".into(), "terminal".into()]);
        let (mut state, _) = step(
            state,
            &Event::Focus {
                app_id: None,
                title: String::new(),
            },
        );
        let button = Event::Button {
            name: ButtonName::West,
            state: ButtonState::Pressed,
        };
        assert_eq!(
            step(state.clone(), &button).1,
            vec![Action::Key {
                key: KeyName::Escape,
                mods: Vec::new()
            }]
        );
        state.replace_contexts(vec![
            parse_context("app_id = 'terminal'\n[buttons]\nwest = { key = 'tab' }").unwrap(),
        ]);
        assert_eq!(
            step(state.clone(), &button).1,
            vec![Action::Key {
                key: KeyName::Tab,
                mods: Vec::new()
            }]
        );
        let (state, _) = step(
            state,
            &Event::Focus {
                app_id: Some("unrelated".into()),
                title: String::new(),
            },
        );
        assert_eq!(step(state, &button).1, Vec::<Action>::new());
    }
    #[test]
    fn an_explicit_close_phrase_keeps_the_active_context_mapping() {
        use tehuti_protocol::KeyName;
        let context = parse_context("app_id = 'editor'\n[buttons]\n[speech.Close]\nkey = 'escape'").unwrap();
        let (state, _) = step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("editor".into()),
                title: String::new(),
            },
        );
        let (_, actions) = step(
            state,
            &Event::Speech {
                text: Text::new("Close.".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Key {
                key: KeyName::Escape,
                mods: Vec::new()
            }]
        );
    }
    #[test]
    fn default_close_precedes_fuzzy_matching_and_unmatched_dictation() {
        let context = parse_context("app_id = 'editor'\nunmatched_speech = 'type'\n[buttons]\n[speech_matching]\nfuzzy = true\n[speech.Closes]\nnotify = 'other'").unwrap();
        let (state, _) = step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("editor".into()),
                title: String::new(),
            },
        );
        assert_eq!(state.fuzzy_speech_match("Close."), None);
        let (_, actions) = step(
            state,
            &Event::Speech {
                text: Text::new("Close.".into()).unwrap(),
            },
        );
        assert_eq!(actions, vec![Action::Close]);
    }
}
