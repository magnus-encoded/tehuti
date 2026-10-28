//! Speech matching through the core routing seam.
#[cfg(test)]
mod fuzzy_speech {
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, Event, Text, parse_context};

    #[test]
    fn opted_in_context_routes_a_folded_norwegian_phrase() {
        let context = parse_context("app_id = '*'\n[buttons]\n[speech]\n'Åpne Bærekraft.' = { launch = 'garden.desktop' }\n[speech_matching]\nfuzzy = true").unwrap();
        let (_, actions) = step(
            State::with_contexts(vec![context]),
            &Event::Speech {
                text: Text::new("Åpne berekraft.".into()).unwrap(),
            },
        );
        assert_eq!(
            actions,
            vec![Action::Launch {
                desktop_entry: Text::new("garden.desktop".into()).unwrap()
            }]
        );
    }

    fn context(app: &str, setting: &str, mappings: &str) -> tehuti_protocol::Context {
        parse_context(&format!(
            "app_id = '{app}'\n[buttons]\n[speech]\n{mappings}\n{setting}"
        ))
        .unwrap()
    }

    fn launch(name: &str) -> Action {
        Action::Launch {
            desktop_entry: Text::new(format!("{name}.desktop")).unwrap(),
        }
    }

    fn route(global: &str, focused: &str, phrase: &str) -> (State, Vec<Action>) {
        let mappings = "'Åpne Bærekraft.' = { launch = 'garden.desktop' }\n'SuperTux.' = { launch = 'game.desktop' }\n'Åpne SuperTux.' = { launch = 'game.desktop' }\n'Lukk vindu.' = { close = 'focused' }\n'Ned.' = { key = 'Down' }";
        let state = State::with_contexts(vec![
            context("*", global, mappings),
            context("editor", focused, ""),
        ]);
        let (state, _) = step(
            state,
            &Event::Focus {
                app_id: Some("editor".into()),
                title: String::new(),
            },
        );
        step(
            state,
            &Event::Speech {
                text: Text::new(phrase.into()).unwrap(),
            },
        )
    }

    #[test]
    fn spaces_and_bare_names_inside_sentences_reach_the_mapping() {
        for phrase in [
            "Åpne Super Tux.",
            "ÅpneSuperTux",
            "Super. Tux.",
            "Hun er supertux.",
        ] {
            let (state, actions) = route("[speech_matching]\nfuzzy = true", "", phrase);
            assert_eq!(actions, vec![launch("game")], "{phrase}");
            let matched = state.fuzzy_speech_match(phrase).unwrap();
            assert_eq!(matched.action, launch("game"));
            assert!(matched.score >= 800);
        }
    }

    #[test]
    fn exact_phrases_win_and_are_not_reported_as_fuzzy() {
        let (state, actions) = route("[speech_matching]\nfuzzy = true", "", "SuperTux!");
        assert_eq!(actions, vec![launch("game")]);
        assert!(state.fuzzy_speech_match("SuperTux!").is_none());
    }

    #[test]
    fn default_and_focused_opt_out_disable_fuzzy_routing() {
        for (global, focused) in [
            ("", ""),
            (
                "[speech_matching]\nfuzzy = true",
                "[speech_matching]\nfuzzy = false",
            ),
        ] {
            let (_, actions) = route(global, focused, "Åpne berekraft.");
            assert!(matches!(actions.as_slice(), [Action::Notice { .. }]));
        }
        let (_, actions) = route("", "[speech_matching]\nfuzzy = true", "Åpne berekraft.");
        assert_eq!(actions, vec![launch("garden")]);
    }

    #[test]
    fn noise_and_short_phrases_do_not_match() {
        for phrase in [
            "Hva?",
            "Nde.",
            "Åpne panseret.",
            "Super.",
            "Tux.",
            "Ikke vinduet.",
        ] {
            let (_, actions) = route("[speech_matching]\nfuzzy = true", "", phrase);
            assert!(
                matches!(actions.as_slice(), [Action::Notice { .. }]),
                "{phrase}: {actions:?}"
            );
        }
    }

    #[test]
    fn fuzzy_mapping_precedes_unmatched_dictation_and_exact_global_precedes_fuzzy() {
        let global = context(
            "*",
            "[speech_matching]\nfuzzy = true",
            "'Åpne Bærekraft.' = { launch = 'garden.desktop' }\n'Åpne berekraft.' = { launch = 'exact.desktop' }",
        );
        let focused = parse_context("app_id = 'editor'\nunmatched_speech = 'type'\n[buttons]\n[speech]\n'SuperTux.' = { launch = 'game.desktop' }").unwrap();
        let (state, _) = step(
            State::with_contexts(vec![global, focused]),
            &Event::Focus {
                app_id: Some("editor".into()),
                title: String::new(),
            },
        );
        for (phrase, expected) in [
            ("Super Tux.", launch("game")),
            ("Åpne berekraft.", launch("exact")),
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

    #[test]
    fn ambiguous_actions_are_rejected_and_destructive_actions_need_nine_tenths() {
        let mappings = "'Åpne Mario.' = { launch = 'one.desktop' }\n'Åpne Maria.' = { launch = 'two.desktop' }\n'Lukk programmet.' = { close = 'focused' }\n'Åpne programmet.' = { launch = 'program.desktop' }";
        let state = State::with_contexts(vec![context("*", "[speech_matching]\nfuzzy = 0.6", mappings)]);
        for (phrase, expected) in [
            ("Åpne Marie.", None),
            ("Lukk progammet.", Some(Action::Close)),
            ("Lukk proggrammer og.", None),
            ("Åpne proggrammer og.", Some(launch("program"))),
        ] {
            let (_, actions) = step(
                state.clone(),
                &Event::Speech {
                    text: Text::new(phrase.into()).unwrap(),
                },
            );
            if let Some(expected) = expected {
                assert_eq!(actions, vec![expected], "{phrase}");
            } else {
                assert!(matches!(actions.as_slice(), [Action::Notice { .. }]), "{phrase}");
            }
        }
    }
    proptest::proptest! {
        #[test]
        fn exact_mappings_keep_focused_priority_at_every_threshold(
            name in "[a-z]{5,16}", threshold in 0..=1000u16,
        ) {
            let global = context("*", &format!("[speech_matching]\nfuzzy = {}.{:03}", threshold.checked_div(1000).unwrap(), threshold.checked_rem(1000).unwrap()),
                &format!("'{name}' = {{ launch = 'global.desktop' }}"));
            let focused = context("editor", "", &format!("'{name}' = {{ launch = 'focused.desktop' }}"));
            let (state, _) = step(State::with_contexts(vec![global, focused]), &Event::Focus {
                app_id: Some("editor".into()), title: String::new(),
            });
            let (_, actions) = step(state.clone(), &Event::Speech { text: Text::new(name.clone()).unwrap() });
            proptest::prop_assert_eq!(actions, vec![launch("focused")]);
            proptest::prop_assert!(state.fuzzy_speech_match(&name).is_none());
        }
    }
}
