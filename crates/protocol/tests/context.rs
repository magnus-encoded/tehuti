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
}
