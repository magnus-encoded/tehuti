//! Examples and rejection rules from the plugin author guide.

#[cfg(test)]
mod contract {

    use tehuti_protocol::{Message, parse_message};

    #[test]
    fn pinned_verbs_reject_unsupported_locales_and_invalid_hints() {
        for entry in [
            serde_json::json!({"word":"orion","locale":"xx"}),
            serde_json::json!({"word":"orion","locale":"en-US"}),
            serde_json::json!({"word":"orion","locale":"EN"}),
            serde_json::json!({"word":"orion","locale":"en","hints":vec!["chat";17]}),
            serde_json::json!({"word":"orion","locale":"en","hints":["bad\0hint"]}),
            serde_json::json!({"word":"two words","locale":"en"}),
            serde_json::json!({"word":"orion","locale":"en","extra":true}),
            serde_json::json!({"word":"orion","hints":[]}),
        ] {
            let notice = serde_json::json!({"jsonrpc":"2.0","method":"verbs","params":{"verbs":[entry]}});
            assert!(
                parse_message(&serde_json::to_vec(&notice).unwrap()).is_err(),
                "{entry}"
            );
            let manifest = serde_json::json!({"kind":"verb","verbs":[entry],"executable":"./verb","app_ids":[],"events":["speech"],"start":"always","permissions":["text"]});
            assert!(
                tehuti_protocol::parse_manifest(&serde_json::to_vec(&manifest).unwrap()).is_err(),
                "{entry}"
            );
        }
        let notice = serde_json::json!({"jsonrpc":"2.0","method":"verbs","params":{"verbs":[{"word":"orion","locale":"en","hints":vec!["chat";16]}]}});
        assert!(parse_message(&serde_json::to_vec(&notice).unwrap()).is_ok());
    }

    #[test]
    fn pinned_verbs_round_trip_in_notices_requests_and_manifests() {
        for value in [
            serde_json::json!({"jsonrpc":"2.0","method":"verbs","params":{"verbs":["ada",{"word":"orion","locale":"en","hints":["chat","channel"]}]}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"verb","params":{"verb":"orion","locale":"en","text":"chat with say"}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"verb","params":{"verb":"ada","text":"hello"}}),
        ] {
            let parsed = parse_message(&serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        }
        let manifest = br#"{"kind":"verb","verbs":["ada",{"word":"orion","locale":"en","hints":["chat"]}],"executable":"./verb","app_ids":[],"events":["speech"],"start":"always","permissions":["text"]}"#;
        assert!(tehuti_protocol::parse_manifest(manifest).is_ok());
    }

    #[test]
    fn every_transcript_line_is_a_valid_protocol_message() {
        let guide = include_str!("../../../docs/protocol/README.md");
        let mut lines = 0;
        for row in guide
            .lines()
            .filter(|line| line.starts_with("| Core →") || line.starts_with("| Plugin →"))
        {
            let source = row.split('`').nth(1).unwrap();
            let _message = parse_message(source.as_bytes()).unwrap();
            lines += 1;
        }
        assert_eq!(lines, 9);
    }

    #[test]
    fn unknown_methods_and_invalid_values_are_rejected() {
        for source in [
            r#"{"jsonrpc":"2.0","id":1,"method":"unknown","params":{}}"#,
            r#"[{"jsonrpc":"2.0","id":1,"result":[]}]"#,
            r#"{"jsonrpc":"2.0","id":"1","result":[]}"#,
            r#"{"jsonrpc":"2.0","id":-1,"result":[]}"#,
            r#"{"jsonrpc":"2.0","id":1.5,"result":[]}"#,
            r#"{"jsonrpc":"2.0","method":"focus","params":{"app_id":null,"title":null}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":[{"type":"text","text":"\u0000"}]}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":[{"type":"text","text":{"entry":"mail"}}]}"#,
        ] {
            assert!(parse_message(source.as_bytes()).is_err(), "{source}");
        }
    }

    #[test]
    fn error_responses_accept_standard_fields_and_reject_invalid_shapes() {
        for source in [
            r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32601,"message":"unknown"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":42,"message":"","data":{"detail":null}}}"#,
        ] {
            let message = parse_message(source.as_bytes()).unwrap();
            assert!(matches!(message, Message::Error(_)));
            assert_eq!(
                parse_message(&serde_json::to_vec(&message).unwrap()).unwrap(),
                message
            );
        }
        for source in [
            r#"{"jsonrpc":"2.0","id":1,"result":[],"error":{"code":-32601,"message":"unknown"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":1.5,"message":"unknown"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":1}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"unknown","extra":true}}"#,
        ] {
            assert!(parse_message(source.as_bytes()).is_err(), "{source}");
        }
    }

    #[test]
    fn empty_text_and_unsigned_integer_ids_are_valid() {
        for source in [
            r#"{"jsonrpc":"2.0","id":18446744073709551615,"result":[{"type":"text","text":""}]}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"actions","params":{"actions":[]}}"#,
        ] {
            assert!(parse_message(source.as_bytes()).is_ok(), "{source}");
        }
        assert!(matches!(
            parse_message(br#"{"jsonrpc":"2.0","id":0,"result":[]}"#).unwrap(),
            Message::Response(_)
        ));
    }

    #[test]
    fn pad_overrides_and_status_are_notices_with_checked_values() {
        for line in [
            r#"{"jsonrpc":"2.0","method":"pad_overrides","params":{"overrides":[{"button":"south","held":["left_bumper"]}]}}"#,
            r#"{"jsonrpc":"2.0","method":"pad_overrides","params":{"overrides":[]}}"#,
            r#"{"jsonrpc":"2.0","method":"status","params":{"text":"Guild"}}"#,
            r#"{"jsonrpc":"2.0","method":"status","params":{"text":null}}"#,
        ] {
            assert!(parse_message(line.as_bytes()).is_ok(), "{line}");
        }
        let many = format!(
            r#"{{"jsonrpc":"2.0","method":"pad_overrides","params":{{"overrides":[{}]}}}}"#,
            vec![r#"{"button":"east"}"#; 65].join(",")
        );
        let long = format!(
            r#"{{"jsonrpc":"2.0","method":"status","params":{{"text":"{}"}}}}"#,
            "x".repeat(33)
        );
        for line in [
            r#"{"jsonrpc":"2.0","method":"pad_overrides","params":{"overrides":[{"button":"guide"}]}}"#,
            r#"{"jsonrpc":"2.0","method":"pad_overrides","params":{"overrides":[{"button":"east","held":["record"]}]}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"pad_overrides","params":{"overrides":[]}}"#,
            r#"{"jsonrpc":"2.0","method":"status","params":{"text":"a\nb"}}"#,
            many.as_str(),
            long.as_str(),
        ] {
            assert!(parse_message(line.as_bytes()).is_err(), "{line}");
        }
    }

    #[test]
    fn invoke_words_are_a_notice_of_slash_command_names() {
        let good = r#"{"jsonrpc":"2.0","method":"invoke.words","params":{"words":["usage","code-review"]}}"#;
        assert!(parse_message(good.as_bytes()).is_ok(), "{good}");
        let many = format!(
            r#"{{"jsonrpc":"2.0","method":"invoke.words","params":{{"words":[{}]}}}}"#,
            vec![r#""word""#; 129].join(",")
        );
        for line in [
            r#"{"jsonrpc":"2.0","method":"invoke.words","params":{"words":["Bad Word"]}}"#,
            r#"{"jsonrpc":"2.0","method":"invoke.words","params":{"words":["a--b"]}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"invoke.words","params":{"words":[]}}"#,
            many.as_str(),
        ] {
            assert!(parse_message(line.as_bytes()).is_err(), "{line}");
        }
    }

    #[test]
    fn speech_language_is_a_notice_of_a_lowercase_language_code() {
        let good = r#"{"jsonrpc":"2.0","method":"speech.language","params":{"language":"no"}}"#;
        assert!(parse_message(good.as_bytes()).is_ok(), "{good}");
        for line in [
            r#"{"jsonrpc":"2.0","method":"speech.language","params":{"language":"No"}}"#,
            r#"{"jsonrpc":"2.0","method":"speech.language","params":{"language":"n"}}"#,
            r#"{"jsonrpc":"2.0","method":"speech.language","params":{"language":"../etc"}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"speech.language","params":{"language":"no"}}"#,
        ] {
            assert!(parse_message(line.as_bytes()).is_err(), "{line}");
        }
    }

    #[test]
    fn toggles_manifest_validates_names_limits_and_reserved() {
        let app_manifest = serde_json::json!({
            "executable": "./toggle.py",
            "app_ids": ["app"],
            "events": [],
            "start": "always",
            "permissions": [],
            "toggles": [
                {"name": "Night Light"},
                {"name": "stream mode", "active": true}
            ]
        });
        let parsed = tehuti_protocol::parse_manifest(&serde_json::to_vec(&app_manifest).unwrap()).unwrap();
        assert_eq!(parsed.toggles.len(), 2);
        assert_eq!(parsed.toggles[0].name(), "night light");
        assert!(!parsed.toggles[0].active(), "active defaults to false");
        assert_eq!(parsed.toggles[1].name(), "stream mode");
        assert!(parsed.toggles[1].active(), "explicit active true is preserved");

        let verb_manifest = serde_json::json!({
            "kind": "verb",
            "verbs": ["ada"],
            "executable": "./verb.py",
            "app_ids": [],
            "events": ["speech"],
            "start": "always",
            "permissions": ["text"],
            "toggles": [{"name": "focus lens"}]
        });
        assert!(
            tehuti_protocol::parse_manifest(&serde_json::to_vec(&verb_manifest).unwrap()).is_ok(),
            "verb plugins may register toggles"
        );

        for bad_toggle in [
            serde_json::json!({"name": ""}),
            serde_json::json!({"name": "   "}),
            serde_json::json!({"name": "stay awake"}),
            serde_json::json!({"name": "Microphone Mute."}),
            serde_json::json!({"name": "DO NOT DISTURB"}),
            serde_json::json!({"name": "on"}),
            serde_json::json!({"name": "off"}),
            serde_json::json!({"name": "lights on"}),
            serde_json::json!({"name": "lights off"}),
            serde_json::json!({"name": "a\nb"}),
            serde_json::json!({"name": "a".repeat(25)}),
            serde_json::json!({"name": "night light", "extra": 1}),
        ] {
            let manifest = serde_json::json!({
                "executable": "./toggle.py",
                "app_ids": [],
                "events": [],
                "start": "always",
                "permissions": [],
                "toggles": [bad_toggle]
            });
            assert!(
                tehuti_protocol::parse_manifest(&serde_json::to_vec(&manifest).unwrap()).is_err(),
                "{manifest}"
            );
        }
    }

    #[test]
    fn toggles_notices_and_manifest_caps_validate_and_round_trip() {
        let duplicate = serde_json::json!({
            "executable": "./toggle.py",
            "app_ids": [],
            "events": [],
            "start": "always",
            "permissions": [],
            "toggles": [{"name": "Night Light"}, {"name": "night light."}]
        });
        assert!(
            tehuti_protocol::parse_manifest(&serde_json::to_vec(&duplicate).unwrap()).is_err(),
            "duplicate normalized toggle names must be rejected"
        );

        let too_many: Vec<_> = (0..33)
            .map(|index| serde_json::json!({"name": format!("mode {index}")}))
            .collect();
        let over_limit = serde_json::json!({
            "executable": "./toggle.py",
            "app_ids": [],
            "events": [],
            "start": "always",
            "permissions": [],
            "toggles": too_many
        });
        assert!(
            tehuti_protocol::parse_manifest(&serde_json::to_vec(&over_limit).unwrap()).is_err(),
            "more than 32 toggles must be rejected"
        );

        for good in [
            r#"{"jsonrpc":"2.0","method":"toggle","params":{"name":"night light","active":true}}"#,
            r#"{"jsonrpc":"2.0","method":"toggles","params":{"toggles":[{"name":"night light","active":false}]}}"#,
        ] {
            let message = parse_message(good.as_bytes()).unwrap();
            assert_eq!(
                parse_message(&serde_json::to_vec(&message).unwrap()).unwrap(),
                message
            );
        }
        for bad in [
            r#"{"jsonrpc":"2.0","id":1,"method":"toggle","params":{"name":"night light","active":true}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"toggles","params":{"toggles":[]}}"#,
            r#"{"jsonrpc":"2.0","method":"toggle","params":{"name":"lights on","active":true}}"#,
            r#"{"jsonrpc":"2.0","method":"toggle","params":{"name":"","active":true}}"#,
            r#"{"jsonrpc":"2.0","method":"toggles","params":{"toggles":[{"name":"stay awake"}]}}"#,
        ] {
            assert!(parse_message(bad.as_bytes()).is_err(), "{bad}");
        }
    }
}
