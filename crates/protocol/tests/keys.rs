//! Keyboard names at the configuration and protocol boundaries.

#[cfg(test)]
mod keys {
    use std::collections::HashSet;
    use tehuti_protocol::{Action, ButtonName, KeyName, parse_context};

    const EXPECTED: &[(&str, u32)] = &[
        ("escape", 1),
        ("enter", 28),
        ("space", 57),
        ("tab", 15),
        ("backspace", 14),
        ("up", 103),
        ("down", 108),
        ("left", 105),
        ("right", 106),
        ("a", 30),
        ("b", 48),
        ("c", 46),
        ("d", 32),
        ("e", 18),
        ("f", 33),
        ("g", 34),
        ("h", 35),
        ("i", 23),
        ("j", 36),
        ("k", 37),
        ("l", 38),
        ("m", 50),
        ("n", 49),
        ("o", 24),
        ("p", 25),
        ("q", 16),
        ("r", 19),
        ("s", 31),
        ("t", 20),
        ("u", 22),
        ("v", 47),
        ("w", 17),
        ("x", 45),
        ("y", 21),
        ("z", 44),
        ("0", 11),
        ("1", 2),
        ("2", 3),
        ("3", 4),
        ("4", 5),
        ("5", 6),
        ("6", 7),
        ("7", 8),
        ("8", 9),
        ("9", 10),
        ("minus", 12),
        ("equal", 13),
        ("left_bracket", 26),
        ("right_bracket", 27),
        ("semicolon", 39),
        ("apostrophe", 40),
        ("grave", 41),
        ("comma", 51),
        ("period", 52),
        ("slash", 53),
        ("backslash", 43),
        ("f1", 59),
        ("f2", 60),
        ("f3", 61),
        ("f4", 62),
        ("f5", 63),
        ("f6", 64),
        ("f7", 65),
        ("f8", 66),
        ("f9", 67),
        ("f10", 68),
        ("f11", 87),
        ("f12", 88),
        ("home", 102),
        ("end", 107),
        ("page_up", 104),
        ("page_down", 109),
        ("insert", 110),
        ("delete", 111),
    ];

    #[test]
    fn every_keyboard_name_round_trips_and_has_a_unique_evdev_code() {
        assert_eq!(EXPECTED.len(), KeyName::ALL.len());
        let mut keys = HashSet::new();
        let mut codes = HashSet::new();
        for (name, code) in EXPECTED.iter().copied() {
            let key: KeyName = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert!(keys.insert(key), "duplicate key {name}");
            assert!(codes.insert(key.evdev_code()), "duplicate evdev code for {name}");
            assert_eq!(key.evdev_code(), code, "{name}");
            assert_eq!(serde_json::to_value(key).unwrap(), serde_json::json!(name));
            let context = parse_context(&format!(
                "app_id = 'test'
[buttons.east]
key = '{name}'"
            ))
            .unwrap();
            assert_eq!(
                context.action_for(ButtonName::East),
                Some(Action::Key { key, mods: vec![] })
            );
        }
        assert!(KeyName::ALL.iter().all(|key| keys.contains(key)));
    }
}
