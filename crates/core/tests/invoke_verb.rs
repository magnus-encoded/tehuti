//! The built-in `invoke` verb through the pure core seam.
#[cfg(test)]
mod invoke_verb {
    use proptest::prelude::*;
    use tehuti_core::invoke::{slash_command, slugify};
    use tehuti_core::{State, step, step_with_plugin};
    use tehuti_protocol::{Action, Event, Text, parse_context};

    fn word() -> impl Strategy<Value = String> {
        "[a-z]{2,8}"
    }

    fn words() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(word(), 1..4)
    }

    fn speech(text: &str) -> Event {
        Event::Speech {
            text: Text::new(text.into()).unwrap(),
        }
    }

    fn focused(source: &str) -> State {
        let context = parse_context(source).unwrap();
        let (state, _) = step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("editor".into()),
                title: String::new(),
            },
        );
        state
    }

    fn typed(text: &str) -> Vec<Action> {
        vec![Action::Text {
            text: Text::new(text.into()).unwrap(),
        }]
    }

    proptest! {
        #[test]
        fn a_known_word_spoken_as_separate_words_types_its_slash_command(
            parts in words(),
            casing in prop::sample::select(vec!["Invoke", "invoke", "INVOKE"]),
            punctuation in prop::sample::select(vec!["", ",", "."]),
        ) {
            let known = parts.join("-");
            let spoken = format!("{casing}{punctuation} {}.", parts.join(" "));
            prop_assert_eq!(
                slash_command(&spoken, [known.as_str()]),
                Some(format!("/{known}")),
                "the spoken words snap to the known word"
            );
        }

        #[test]
        fn words_no_context_knows_are_slugified_as_heard(parts in words()) {
            let spoken = format!("invoke {}", parts.join(" "));
            prop_assert_eq!(
                slash_command(&spoken, []),
                Some(format!("/{}", parts.join("-"))),
                "unknown words become a hyphenated slug"
            );
        }

        #[test]
        fn speech_that_does_not_begin_with_the_verb_is_not_invoked(
            first in word().prop_filter("not the verb", |first| !first.starts_with("inv")),
            parts in words(),
        ) {
            let spoken = format!("{first} {}", parts.join(" "));
            prop_assert_eq!(slash_command(&spoken, []), None, "only the verb invokes");
        }

        #[test]
        fn the_built_in_verb_wins_over_plugin_verbs_and_context_phrases(
            parts in words(),
            plugin_active in any::<bool>(),
        ) {
            let spoken = format!("invoke {}", parts.join(" "));
            let state = focused(&format!(
                "app_id = 'editor'\n[buttons]\n[speech]\n'{spoken}' = {{ text = 'shadowed' }}"
            ));
            let (_, actions, routed) = step_with_plugin(state, &speech(&spoken), plugin_active);
            prop_assert_eq!(actions, typed(&format!("/{}", parts.join("-"))), "the verb types the slash command");
            prop_assert!(!routed, "no plugin hears an invoked command");
        }
    }

    #[test]
    fn a_misheard_long_word_snaps_to_the_closest_known_word() {
        let state = focused("app_id = 'editor'\ninvoke = ['usage', 'chief-of-staff']\n[buttons]");
        let (state, snapped) = step(state, &speech("Invoke chief of stuff."));
        assert_eq!(snapped, typed("/chief-of-staff"), "a near miss snaps");
        let (_, exact) = step(state, &speech("invoke usage"));
        assert_eq!(exact, typed("/usage"), "an exact word types itself");
    }

    #[test]
    fn short_words_snap_only_on_an_equal_sound() {
        assert_eq!(
            slash_command("invoke used", ["usage"]),
            Some("/used".into()),
            "no fuzzy snap under five letters"
        );
        assert_eq!(
            slugify("Usage, Report!"),
            "usage-report",
            "punctuation splits words"
        );
    }

    #[test]
    fn a_lone_verb_falls_through_to_context_phrases() {
        let state = focused("app_id = 'editor'\n[buttons]\n[speech]\ninvoke = { text = 'plain' }");
        let (_, actions) = step(state, &speech("invoke"));
        assert_eq!(actions, typed("plain"), "the verb needs words after it");
    }

    #[test]
    fn invoke_words_must_be_slash_command_names() {
        for bad in ["Usage", "usage report", "-usage", "usage--report", ""] {
            assert!(
                parse_context(&format!("app_id = 'editor'\ninvoke = ['{bad}']\n[buttons]")).is_err(),
                "{bad:?} is rejected"
            );
        }
        let context = parse_context("app_id = 'editor'\ninvoke = ['usage-report', 'v2']\n[buttons]").unwrap();
        assert_eq!(
            context.invoke_words(),
            ["usage-report", "v2"],
            "valid words load in order"
        );
    }
}
