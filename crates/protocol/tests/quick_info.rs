#[cfg(test)]
mod quick_info {
    use tehuti_protocol::{Action, ButtonName, Stick, StickDirection, parse_context};

    #[test]
    fn info_cycle_parses_in_buttons_and_sticks() {
        let context = parse_context(
            "app_id = '*'\n[buttons]\npaddle_1 = { info = 'cycle' }\n\
             [sticks.right]\nup = { info = 'cycle' }\n",
        )
        .unwrap();
        assert_eq!(context.action_for(ButtonName::Paddle1), Some(Action::CycleInfo));
        assert_eq!(
            context
                .stick_mapping(Stick::Right, StickDirection::Up, None)
                .map(|mapping| &mapping.action),
            Some(&Action::CycleInfo)
        );
    }

    #[test]
    fn info_cycle_is_rejected_in_speech() {
        let error = parse_context("app_id = '*'\n[speech]\n'next info' = { info = 'cycle' }\n").unwrap_err();
        assert!(
            error.contains("steps belong on the controller"),
            "expected ADR 0008 error, got: {error}"
        );
    }

    #[test]
    fn unknown_info_value_is_rejected() {
        let error = parse_context("app_id = '*'\n[buttons]\nwest = { info = 'show' }\n").unwrap_err();
        assert!(!error.is_empty(), "unknown info value must fail parsing");
    }

    #[test]
    fn cycle_info_action_roundtrips_as_json() {
        let encoded = serde_json::to_string(&Action::CycleInfo).unwrap();
        let decoded: Action = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, Action::CycleInfo);
    }
}
