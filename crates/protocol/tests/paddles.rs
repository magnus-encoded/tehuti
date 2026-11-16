//! Paddle buttons resolve to their own names.

#[cfg(test)]
mod paddles {
    use tehuti_protocol::parse_context;

    #[test]
    fn each_paddle_name_resolves_to_its_own_button() {
        for num in 1..=4 {
            let talk = parse_context(&format!(
                "app_id = 'test'\nbuttons = {{}}\n[pad]\npush_to_talk = ['paddle_{num}']"
            ))
            .unwrap();
            assert_eq!(format!("{:?}", talk.passthrough_push_to_talk()), format!("[Paddle{num}]"));
        }
    }
}
