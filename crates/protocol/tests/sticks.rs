//! `[sticks]` parses at the configuration boundary.

#[cfg(test)]
mod sticks {
    use proptest::prelude::*;
    use tehuti_protocol::{Action, ButtonName, KeyName, Stick, StickDirection, parse_context};

    const BASE: &str = "app_id = 'org.example.Player'\n[buttons]\n";

    fn key(key: KeyName) -> Action {
        Action::Key {
            key,
            mods: Vec::new(),
        }
    }

    #[test]
    fn the_issue_example_parses_with_modifiers_and_default_repeat() {
        let context = parse_context(&format!(
            "{BASE}[sticks]\nleft.left = {{ key = 'left' }}\nleft.up = {{ key = 'up', with = 'north' }}\nright.up = {{ key = '0', with = 'east' }}\n"
        ))
        .unwrap();
        let left = context
            .stick_mapping(Stick::Left, StickDirection::Left, None)
            .unwrap();
        assert_eq!(left.action, key(KeyName::Left), "left.left sends Left");
        assert!(left.repeat, "sticks repeat by default");
        assert!(
            context
                .stick_mapping(Stick::Left, StickDirection::Up, None)
                .is_none(),
            "a modified entry is not in the unmodified layer"
        );
        assert_eq!(
            context
                .stick_mapping(Stick::Left, StickDirection::Up, Some(ButtonName::North))
                .map(|mapping| mapping.action.clone()),
            Some(key(KeyName::Up)),
        );
        assert!(context.stick_modifier(ButtonName::North), "north is a modifier");
        assert!(context.stick_modifier(ButtonName::East), "east is a modifier");
        assert!(!context.stick_modifier(ButtonName::South), "south is not");
        assert!(context.maps_stick(Stick::Right), "right stick is mapped");
    }

    #[test]
    fn a_context_without_sticks_maps_neither_stick() {
        let context = parse_context(BASE).unwrap();
        assert!(!context.maps_stick(Stick::Left), "left stays in mouse mode");
        assert!(!context.maps_stick(Stick::Right), "right stays in mouse mode");
    }

    #[test]
    fn diagonals_use_kebab_case_keys() {
        let context = parse_context(&format!(
            "{BASE}[sticks]\nright.up-right = {{ key = 'page_up', repeat = false }}\n"
        ))
        .unwrap();
        let mapping = context
            .stick_mapping(Stick::Right, StickDirection::UpRight, None)
            .unwrap();
        assert!(!mapping.repeat, "repeat = false is kept");
        assert!(parse_context(&format!("{BASE}[sticks]\nright.up_right = {{ key = 'up' }}\n")).is_err());
        assert!(parse_context(&format!("{BASE}[sticks]\nright.north = {{ key = 'up' }}\n")).is_err());
        assert!(parse_context(&format!("{BASE}[sticks]\nmiddle.up = {{ key = 'up' }}\n")).is_err());
    }

    #[test]
    fn one_direction_takes_a_list_of_layers() {
        let context = parse_context(&format!(
            "{BASE}[sticks]\nleft.up = [{{ key = 'up' }}, {{ key = 'page_up', with = 'west' }}]\n"
        ))
        .unwrap();
        assert!(
            context
                .stick_mapping(Stick::Left, StickDirection::Up, None)
                .is_some(),
            "the unmodified layer"
        );
        assert!(
            context
                .stick_mapping(Stick::Left, StickDirection::Up, Some(ButtonName::West))
                .is_some(),
            "the west layer"
        );
        assert!(
            parse_context(&format!(
                "{BASE}[sticks]\nleft.up = [{{ key = 'up' }}, {{ key = 'down' }}]\n"
            ))
            .is_err(),
            "two entries for the same layer"
        );
    }

    #[test]
    fn invalid_entries_fail_to_load() {
        for entry in [
            "{ key = 'up', hold = true }",
            "{ key = 'up', with = 'left_bumper' }",
            "{ key = 'up', with = 'start' }",
            "{ key = 'up', speed = 3 }",
            "{ key = 'nonsense' }",
        ] {
            assert!(
                parse_context(&format!("{BASE}[sticks]\nleft.up = {entry}\n")).is_err(),
                "{entry} must fail"
            );
        }
    }

    proptest! {
        #[test]
        fn every_stick_direction_and_face_modifier_parses(
            stick in prop::sample::select(vec![("left", Stick::Left), ("right", Stick::Right)]),
            direction in prop::sample::select(StickDirection::ALL.to_vec()),
            with in prop::sample::select(vec![
                (None, None),
                (Some("south"), Some(ButtonName::South)),
                (Some("east"), Some(ButtonName::East)),
                (Some("west"), Some(ButtonName::West)),
                (Some("north"), Some(ButtonName::North)),
            ]),
        ) {
            let name = toml::Value::try_from(direction).unwrap();
            let name = name.as_str().unwrap();
            let modifier = with.0.map_or_else(String::new, |button| format!(", with = '{button}'"));
            let context = parse_context(&format!(
                "{BASE}[sticks]\n{}.{name} = {{ text = 'go'{modifier} }}\n",
                stick.0
            ))
            .unwrap();
            prop_assert!(context.stick_mapping(stick.1, direction, with.1).is_some());
            prop_assert_eq!(context.stick_mappings().len(), 1);
        }
    }
}
