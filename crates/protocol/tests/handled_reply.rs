#[cfg(test)]
mod handled_reply {
    use tehuti_protocol::parse_message;

    fn parsed(result: &str) -> String {
        let line = format!(r#"{{"jsonrpc":"2.0","id":7,"result":{result}}}"#);
        format!("{:?}", parse_message(line.as_bytes()))
    }

    #[test]
    fn handled_true_is_its_own_result() {
        let text = parsed(r#"{"handled":true}"#);
        assert!(text.starts_with("Ok(Response"), "{text}");
        assert!(text.contains("Handled"), "{text}");
    }

    #[test]
    fn handled_false_or_extra_fields_are_rejected() {
        for result in [
            r#"{"handled":false}"#,
            r#"{"handled":true,"extra":1}"#,
            r#"{"handled":"yes"}"#,
        ] {
            let text = parsed(result);
            assert!(text.starts_with("Err("), "{result}: {text}");
        }
    }

    #[test]
    fn an_action_list_is_still_an_action_list() {
        let text = parsed("[]");
        assert!(text.contains("Actions"), "{text}");
    }
}
