//! Configuration boundary properties.

#[cfg(test)]
mod context {
    use proptest::prelude::*;
    use tehuti_protocol::{KeyName, parse_context};

    proptest! {
        #[test]
        fn arbitrary_configuration_returns_a_result(source in any::<String>()) {
            let _parsed = parse_context(&source);
        }

        #[test]
        fn every_named_key_parses(key in prop::sample::select(vec![
            ("enter", KeyName::Enter), ("escape", KeyName::Escape),
            ("space", KeyName::Space), ("tab", KeyName::Tab),
            ("backspace", KeyName::Backspace), ("up", KeyName::Up),
            ("down", KeyName::Down), ("left", KeyName::Left), ("right", KeyName::Right),
        ])) {
            let context = parse_context(&format!("app_id = 'test'\n[buttons.east]\nkey = '{}'", key.0)).unwrap();
            prop_assert_eq!(context.key_for(tehuti_protocol::ButtonName::East).unwrap(), key.1);
        }

        /// Whisper capitalises and punctuates between words as well as at the end.
        #[test]
        fn whisper_punctuation_and_case_do_not_block_a_phrase(
            words in prop::collection::vec("[a-z]{1,8}", 1..5),
            separators in prop::collection::vec(prop::sample::select(vec![" ", ". ", ", ", "! ", "? ", " - ", "  "]), 4),
            ending in prop::sample::select(vec!["", ".", "!", "?", "...", " ."]),
        ) {
            let mapped = words.join(" ");
            let mut spoken = String::new();
            let leading = core::iter::once("").chain(separators.iter().copied());
            for (separator, word) in leading.zip(&words) {
                spoken.push_str(separator);
                let mut letters = word.chars();
                if let Some(first) = letters.next() {
                    spoken.extend(first.to_uppercase());
                    spoken.extend(letters);
                }
            }
            spoken.push_str(ending);
            let context = parse_context(&format!("app_id = 'test'\n[buttons]\n[speech]\n\"{mapped}\" = {{ key = 'enter' }}")).unwrap();
            prop_assert!(context.speech_action(&format!(" {spoken}")).is_some(), "{spoken:?} should match {mapped:?}");
        }
    }

    #[test]
    fn transcript_with_inner_punctuation_matches() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"Front center.\" = { key = 'enter' }")
                .unwrap();
        assert!(context.speech_action("Front. Center.").is_some());
        assert!(context.speech_action("Front centre.").is_none());
        assert!(context.speech_action("Don't.").is_none());
    }

    #[test]
    fn speech_phrases_that_normalize_alike_are_rejected() {
        let error = parse_context(
            "app_id = 'foot'\n[buttons]\n[speech]\n\"Hello\" = { key = 'enter' }\n\"hello.\" = { key = 'tab' }",
        )
        .unwrap_err();
        assert!(error.contains("normalizes to \"hello\""), "{error}");
    }

    #[test]
    fn speech_phrases_that_normalize_to_nothing_are_rejected() {
        for phrase in ["", "...", " ' ", "\u{2014}\u{ff01}"] {
            let source = format!("app_id = 'foot'\n[buttons]\n[speech]\n{phrase:?} = {{ key = 'enter' }}");
            let error = parse_context(&source).unwrap_err();
            assert!(error.contains("no words"), "{phrase:?}: {error}");
        }
    }

    #[test]
    fn empty_transcript_never_matches() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"go\" = { key = 'enter' }").unwrap();
        for spoken in ["", "  ", "...", "\u{3002}"] {
            assert!(context.speech_action(spoken).is_none(), "{spoken:?}");
        }
    }

    #[test]
    fn unicode_apostrophes_and_punctuation_fold() {
        let context = parse_context(
            "app_id = 'foot'\n[buttons]\n[speech]\n\"don't stop\" = { key = 'enter' }\n\"caf\u{e9} ol\u{e9}\" = { key = 'tab' }",
        )
        .unwrap();
        assert!(context.speech_action("Don\u{2019}t stop.").is_some());
        assert!(
            context
                .speech_action("don\u{2bc}t \u{201c}stop\u{201d}")
                .is_some()
        );
        assert!(
            context
                .speech_action("\u{bf}Caf\u{e9}\u{2014}ol\u{e9}!")
                .is_some()
        );
        assert!(context.speech_action("dont stop").is_none());
    }

    #[test]
    fn decomposed_accents_match_composed_ones() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"caf\u{e9}\" = { key = 'enter' }").unwrap();
        assert!(context.speech_action("Cafe\u{301}.").is_some());
        assert!(context.speech_action("cafe").is_none());
        assert_eq!(tehuti_protocol::normalize_phrase("cafe\u{301}"), "caf\u{e9}");
    }

    /// Lowercasing "J\u{30c}" yields "j\u{30c}", which only composes to "\u{1f0}" if NFC runs last.
    #[test]
    fn nfc_applies_after_lowercasing() {
        let context =
            parse_context("app_id = 'foot'\n[buttons]\n[speech]\n\"\u{1f0}\" = { key = 'enter' }").unwrap();
        assert!(context.speech_action("J\u{30c}").is_some());
        let error = parse_context(
            "app_id = 'foot'\n[buttons]\n[speech]\n\"\u{1f0}\" = { key = 'enter' }\n\"J\u{30c}\" = { key = 'tab' }",
        )
        .unwrap_err();
        assert!(error.contains("normalizes to"), "{error}");
    }

    #[test]
    fn text_rejects_nul_at_construction_and_deserialization() {
        assert!(tehuti_protocol::Text::new("a\0b".into()).is_err());
        let source = r#"app_id = 'test'
[buttons.east]
text = "a\u0000b"
"#;
        assert!(parse_context(source).is_err());
        assert!(parse_context(&source.replace("\\u0000", "\\u0001")).is_ok());
        assert_eq!(tehuti_protocol::Text::new("🦀".into()).unwrap().as_str(), "🦀");
    }

    #[test]
    fn context_can_request_a_secret_by_reference() {
        let context = parse_context(
            "app_id = 'browser'\n[buttons.east]\nsecret = { entry = 'password-store/browser' }\n",
        )
        .unwrap();

        assert!(context.key_for(tehuti_protocol::ButtonName::East).is_none());
        assert_eq!(
            context.action_for(tehuti_protocol::ButtonName::East),
            Some(tehuti_protocol::Action::SecretRequest {
                reference: tehuti_protocol::SecretReference {
                    entry: "password-store/browser".into()
                }
            }),
        );
    }

    #[test]
    fn malformed_contexts_are_rejected() {
        for source in [
            "app_id = ''\n[buttons]",
            "app_id = '  '\n[buttons]",
            "app_id = 'test'\n[buttons.unknown]\nkey = 'enter'",
            "app_id = 'test'\n[buttons.east]\nkey = 'unknown'",
            "app_id = 'test'\nextra = true\n[buttons]",
            "app_id = 'test'\n[buttons.east]\nkey = 'enter'\ntext = 'secret'",
            "app_id = 'test'\n[buttons.east]\ntext = '\0'",
            "not toml",
        ] {
            assert!(
                parse_context(source).is_err(),
                "invalid input must fail: {source}"
            );
        }
    }

    #[test]
    fn keyboard_options_reject_unknown_modes_buttons_and_fields() {
        for setting in [
            "on_text_field = 'always'",
            "button = 'unknown'",
            "button = 'guide'",
            "button = 'record'",
            "popup = true",
        ] {
            assert!(
                tehuti_protocol::parse_context(&format!(
                    "app_id = 'editor'\n[buttons]\n[keyboard]\n{setting}"
                ))
                .is_err()
            );
        }
    }
}
