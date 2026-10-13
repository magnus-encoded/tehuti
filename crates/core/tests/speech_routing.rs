#[cfg(test)]
mod speech_routing {
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
            vec![Action::Key { key: KeyName::Enter }],
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
                context.speech_action(heard),
                Some(Action::Key { key: KeyName::Enter }),
                "{heard:?} matches the mapped phrase"
            );
        }
        assert_eq!(
            context.speech_action("hello there"),
            None,
            "other words still differ"
        );
    }
}
