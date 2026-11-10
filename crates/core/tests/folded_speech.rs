//! Order of exact, folded and plural phrase matches through the core seam.
#[cfg(test)]
mod folded_speech {
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, Event, KeyName, Text, parse_context};

    fn key(key: KeyName) -> Action {
        Action::Key {
            key,
            mods: Vec::new(),
        }
    }

    fn route(global: &str, focused: &str, phrase: &str) -> Vec<Action> {
        let context = |app: &str, mappings: &str| {
            parse_context(&format!("app_id = '{app}'\n[buttons]\n[speech]\n{mappings}")).unwrap()
        };
        let state = State::with_contexts(vec![context("*", global), context("editor", focused)]);
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
        .1
    }

    #[test]
    fn a_global_exact_phrase_beats_a_focused_folded_one() {
        let exact = route(
            "'Bay 3' = { key = 'Tab' }",
            "'Bay-three' = { key = 'Enter' }",
            "Bay 3.",
        );
        assert_eq!(exact, vec![key(KeyName::Tab)]);
        let folded = route(
            "'Bay 3' = { key = 'Tab' }",
            "'Bay-three' = { key = 'Enter' }",
            "bay3",
        );
        assert_eq!(folded, vec![key(KeyName::Enter)], "folded tries focused first");
    }

    #[test]
    fn built_in_close_beats_a_focused_folded_phrase() {
        let actions = route("", "'Clo-se' = { key = 'Enter' }", "Close.");
        assert_eq!(actions, vec![Action::Close]);
    }

    #[test]
    fn a_global_folded_phrase_beats_a_focused_plural() {
        let actions = route(
            "'Bay-three' = { key = 'Tab' }",
            "'Bay threes' = { key = 'Enter' }",
            "bay 3",
        );
        assert_eq!(actions, vec![key(KeyName::Tab)]);
    }
}
