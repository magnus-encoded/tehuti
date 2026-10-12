//! Laws of the JSON boundary over generated protocol values and arbitrary bytes.

#[cfg(test)]
mod json {
    use proptest::prelude::*;
    use tehuti_protocol::*;

    fn round_trip<Value>(value: &Value)
    where
        Value: serde::Serialize + serde::de::DeserializeOwned + PartialEq + core::fmt::Debug,
    {
        let bytes = serde_json::to_vec(value).unwrap();
        let decoded: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&decoded, value, "JSON preserves the protocol value");
    }

    proptest! {
        #[test]
        fn json_preserves_every_event_action_and_manifest(
            event in any::<Event>(), action in any::<Action>(),
            executable in "[a-z/.-]{1,80}", app_ids in prop::collection::vec("[a-z.]{1,40}", 0..8),
            subscriptions in prop::collection::vec(any::<bool>(), 0..8),
            permissions in prop::collection::vec(0u8..3, 0..8), always in any::<bool>(),
            plugin_id in prop::option::of(any::<String>()), author in prop::option::of(any::<String>()),
            security_contact in prop::option::of(any::<String>()),
            entry in any::<String>(), version in any::<u64>(), request_id in any::<u64>(),
        ) {
            round_trip(&event);
            round_trip(&action);
            round_trip(&SecretReference { entry });
            let events: Vec<_> = subscriptions.into_iter().map(|focus| if focus { Subscription::Focus } else { Subscription::Button }).collect();
            let manifest = Manifest {
                plugin_id, author, security_contact, executable, app_ids, events: events.clone(),
                start: if always { StartPolicy::Always } else { StartPolicy::OnFocus },
                permissions: permissions.into_iter().map(|permission| match permission { 0 => Permission::Key, 1 => Permission::Text, _ => Permission::Quit }).collect(),
            };
            prop_assert_eq!(parse_manifest(&serde_json::to_vec(&manifest).unwrap()).unwrap(), manifest);
            let initialized = Initialized { protocol_version: version, encoding: "json".into(), events };
            let event_call = match event {
                Event::Button { name, state } => Some(Call { jsonrpc: RpcVersion::Two, request_id: Some(request_id), method: Method::Button { name, state } }),
                Event::Focus { app_id, title } => Some(Call { jsonrpc: RpcVersion::Two, request_id: None, method: Method::Focus { app_id, title } }),
                Event::Speech { text } => Some(Call { jsonrpc: RpcVersion::Two, request_id: Some(request_id), method: Method::Speech { text } }),
                Event::Tick { .. } => None,
            };
            if let Some(call) = event_call {
                let message = Message::Call(call);
                prop_assert_eq!(parse_message(&serde_json::to_vec(&message).unwrap()).unwrap(), message);
            }
            let messages = vec![
                Message::Call(Call { jsonrpc: RpcVersion::Two, request_id: Some(0), method: Method::Initialize(Initialize { protocol_version: version, encoding: "json".into(), core_version: "test".into(), options: Some(std::collections::BTreeMap::new()) }) }),
                Message::Response(Response { jsonrpc: RpcVersion::Two, request_id: 0, result: ResultValue::Initialized(initialized) }),
                Message::Response(Response { jsonrpc: RpcVersion::Two, request_id, result: ResultValue::Actions(vec![action.clone()]) }),
                Message::Call(Call { jsonrpc: RpcVersion::Two, request_id: None, method: Method::Actions { actions: vec![action] } }),
                Message::Call(Call { jsonrpc: RpcVersion::Two, request_id: None, method: Method::Focus { app_id: Some("test".into()), title: String::new() } }),
            ];
            for message in messages {
                prop_assert_eq!(parse_message(&serde_json::to_vec(&message).unwrap()).unwrap(), message);
            }
        }

        #[test]
        fn arbitrary_bytes_return_results_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..100_000)) {
            let _message = parse_message(&bytes);
            let _manifest = parse_manifest(&bytes);
        }
    }

    #[test]
    fn unknown_plugin_fields_are_named_and_invalid_shapes_rejected() {
        for source in [
            r#"{"jsonrpc":"2.0","id":1,"result":[{"type":"key","key":"tab","secret_value":"password"}]}"#,
            r#"{"jsonrpc":"2.0","id":0,"result":{"protocol_version":0,"encoding":"json","events":[],"secret_value":"password"}}"#,
            r#"{"jsonrpc":"2.0","method":"actions","params":{"actions":[],"secret_value":"password"}}"#,
        ] {
            assert!(
                parse_message(source.as_bytes())
                    .unwrap_err()
                    .contains("secret_value"),
                "unknown field must be named: {source}"
            );
        }
        for source in [
            r#"{"jsonrpc":"2.0","method":"button","params":{"name":"east","state":"pressed"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"focus","params":{"app_id":null,"title":""}}"#,
            r#"{"jsonrpc":"1.0","id":1,"result":[]}"#,
        ] {
            assert!(
                parse_message(source.as_bytes()).is_err(),
                "invalid JSON-RPC: {source}"
            );
        }
        assert!(
            serde_json::from_str::<SecretReference>(r#"{"entry":"mail","value":"password"}"#).is_err(),
            "secret references cannot carry a value"
        );
    }

    #[test]
    fn manifest_metadata_is_optional_and_unknown_fields_are_rejected() {
        let source =
            br#"{"executable":"plugin.py","app_ids":["test"],"events":[],"start":"always","permissions":[]}"#;
        let manifest = tehuti_protocol::parse_manifest(source).unwrap();
        assert_eq!(manifest.plugin_id, None);
        assert_eq!(manifest.author, None);
        assert_eq!(manifest.security_contact, None);
        let mut value: serde_json::Value = serde_json::from_slice(source).unwrap();
        value["unrecognized"] = true.into();
        assert!(
            tehuti_protocol::parse_manifest(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .contains("unrecognized")
        );
    }
}
