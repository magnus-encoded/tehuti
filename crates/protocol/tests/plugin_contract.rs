//! Examples and rejection rules from the plugin author guide.

#[cfg(test)]
mod contract {

    use tehuti_protocol::{Message, parse_message};

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
}
