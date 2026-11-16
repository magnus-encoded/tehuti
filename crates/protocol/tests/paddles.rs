//! Paddle buttons resolve to their typed names.

#[cfg(test)]
mod paddles {
    use tehuti_protocol::{Action, ButtonName, parse_context};

    #[test]
    fn each_paddle_name_parses_in_buttons_and_push_to_talk() {
        for (num, name) in [
            (1, ButtonName::Paddle1),
            (2, ButtonName::Paddle2),
            (3, ButtonName::Paddle3),
            (4, ButtonName::Paddle4),
        ] {
            let bound = parse_context(&format!(
                "app_id = 'test'\n[buttons]\npaddle_{num} = {{ ticker = 'toggle' }}"
            ))
            .unwrap();
            assert_eq!(bound.action_for(name), Some(Action::ToggleTicker));
            let talk = parse_context(&format!(
                "app_id = 'test'\nbuttons = {{}}\n[pad]\npush_to_talk = ['paddle_{num}']"
            ))
            .unwrap();
            assert_eq!(talk.passthrough_push_to_talk(), &[name]);
        }
    }
}
