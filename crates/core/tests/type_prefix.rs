#[cfg(test)]
mod type_prefix {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, Event, KeyName, Text, parse_context};

    fn say(contexts: &[&str], focus: &str, phrase: &str) -> Vec<Action> {
        let contexts = contexts
            .iter()
            .map(|source| parse_context(source).unwrap_or_else(|error| panic!("context rejected: {error}")))
            .collect();
        let (state, _) = step(
            State::with_contexts(contexts),
            &Event::Focus {
                app_id: Some(focus.into()),
                title: String::new(),
            },
        );
        let text = Text::new(phrase.into()).unwrap();
        step(state, &Event::Speech { text }).1
    }

    fn typed(text: &str) -> Vec<Action> {
        vec![Action::Text {
            text: Text::new(text.into()).unwrap(),
        }]
    }

    const GLOBAL: &str = "app_id = '*'\n[buttons]\n[speech]\n'type' = { text_after = true }";
    const APP: &str = "app_id = 'terminal'\n[buttons]\n[speech]";

    #[test]
    fn prefix_types_the_rest_with_its_case_and_punctuation() {
        assert_eq!(
            say(&[GLOBAL, APP], "terminal", "Type hello, World."),
            typed("hello, World. ")
        );
    }

    #[test]
    fn bare_key_types_nothing() {
        let actions = say(&[GLOBAL, APP], "terminal", "Type.");
        assert!(matches!(actions.as_slice(), [Action::Notice { .. }]), "{actions:?}");
    }

    #[test]
    fn rule_in_the_focused_context_works_without_a_global_one() {
        let app = "app_id = 'terminal'\n[buttons]\n[speech]\n'type' = { text_after = true }";
        assert_eq!(say(&[app], "terminal", "type hello"), typed("hello "));
    }

    #[test]
    fn context_opts_out_with_none() {
        let app = "app_id = 'terminal'\n[buttons]\n[speech]\n'type' = { none = true }";
        let actions = say(&[GLOBAL, app], "terminal", "Type hello, World.");
        assert!(matches!(actions.as_slice(), [Action::Notice { .. }]), "{actions:?}");
    }

    #[test]
    fn whole_phrase_rule_wins_over_prefix() {
        let app = "app_id = 'terminal'\n[buttons]\n[speech]\n'type slash' = { key = 'enter' }";
        assert_eq!(
            say(&[GLOBAL, app], "terminal", "Type slash."),
            vec![Action::Key {
                key: KeyName::Enter,
                mods: Vec::new()
            }]
        );
    }

    proptest! {
        #[test]
        fn any_sentence_after_the_key_is_typed_verbatim(
            sentence in "[A-Z][a-z]{0,8}( [a-z]{1,8}){0,3}[.!]?",
            key in prop_oneof![Just("type"), Just("Type"), Just("TYPE")],
        ) {
            prop_assert_eq!(
                say(&[GLOBAL, APP], "terminal", &format!("{key} {sentence}")),
                typed(&format!("{sentence} "))
            );
        }
    }
}
