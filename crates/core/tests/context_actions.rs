//! Context action routing across generated button and phrase mappings.

#[cfg(test)]
mod context_actions {
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, Text, parse_context};

    proptest! {
        #[test]
        fn phrases_and_buttons_route_application_actions(
            app_id in "[a-z]{1,20}", phrase in "[a-z]{1,20}", value in any::<Text>(),
            kind in 0_u8..3,
            button in prop_oneof![Just(ButtonName::East), Just(ButtonName::West), Just(ButtonName::North)],
        ) {
            let (field, mapped, expected) = match kind {
                0 => ("launch", value.as_str(), Action::Launch { desktop_entry: value.clone() }),
                1 => ("close", "focused", Action::Close),
                _ => ("notify", value.as_str(), Action::Notify { text: value.clone() }),
            };
            let mapping = toml::Value::Table(toml::Table::from_iter([
                (field.into(), toml::Value::String(mapped.into())),
            ]));
            let button_name = toml::Value::try_from(button).unwrap().as_str().unwrap().to_owned();
            let source = toml::Table::from_iter([
                ("app_id".into(), toml::Value::String(app_id.clone())),
                ("buttons".into(), toml::Value::Table(toml::Table::from_iter([(button_name, mapping.clone())]))),
                ("speech".into(), toml::Value::Table(toml::Table::from_iter([(phrase.clone(), mapping)]))),
            ]).to_string();
            let context = parse_context(&source).unwrap();
            prop_assert!(context.key_for(button).is_none());
            prop_assert!(tehuti_core::plugin::validate_actions(vec![expected.clone()], &[
                tehuti_protocol::Permission::Key, tehuti_protocol::Permission::Text,
                tehuti_protocol::Permission::Quit,
            ]).is_err(), "application actions remain core-only");
            let (state, _) = step(State::with_contexts(vec![context]), &Event::Focus { app_id: Some(app_id), title: String::new() });
            for event in [Event::Button { name: button, state: ButtonState::Pressed }, Event::Speech { text: Text::new(phrase).unwrap() }] {
                let (_, actions) = step(state.clone(), &event);
                prop_assert_eq!(actions, vec![expected.clone()]);
            }
        }
    }
}
