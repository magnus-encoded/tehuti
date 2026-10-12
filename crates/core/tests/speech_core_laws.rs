//! Speech routing properties across generated context configurations.

#[cfg(test)]
mod speech_core_laws {
    use proptest::prelude::*;
    use tehuti_core::{State, step, step_with_plugin};
    use tehuti_protocol::{Action, Event, Text, parse_context};

    proptest! {
        #[test]
        fn only_focused_context_can_handle_its_generated_phrase(
            phrase in "[a-z]{1,30}",
            output in any::<Text>(),
            app_id in "[a-z]{1,30}",
            matching_focus in any::<bool>(),
            other_phrases in prop::collection::hash_map("other[a-z]{1,20}", any::<Text>(), 0..20),
        ) {
            let mappings: toml::map::Map<String, toml::Value> = other_phrases.into_iter()
                .chain(core::iter::once((phrase.clone(), output.clone())))
                .map(|(key, text)| (key, toml::Value::Table(toml::map::Map::from_iter([
                    ("text".into(), toml::Value::String(text.as_str().into())),
                ])))).collect();
            let source = toml::Table::from_iter([
                ("app_id".into(), toml::Value::String(app_id.clone())),
                ("buttons".into(), toml::Value::Table(toml::Table::new())),
                ("speech".into(), toml::Value::Table(mappings)),
            ]).to_string();
            let context = parse_context(&source).unwrap();
            let (state, _) = step(State::with_contexts(vec![context]), &Event::Focus {
                app_id: matching_focus.then_some(app_id), title: String::new(),
            });
            let speech = Event::Speech { text: Text::new(phrase).unwrap() };
            let (_, actions) = step(state.clone(), &speech);
            if matching_focus {
                prop_assert_eq!(actions, vec![Action::Text { text: output }]);
            } else {
                prop_assert!(matches!(actions.as_slice(), [Action::Notice { .. }]), "unmatched speech has one notice");
            }
            let (_, plugin_actions, forwarded) = step_with_plugin(state, &speech, true);
            prop_assert!(forwarded);
            prop_assert!(plugin_actions.is_empty(), "plugin consumes speech before declarative or fallback output");
        }
    }
}
