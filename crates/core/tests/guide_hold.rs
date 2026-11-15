#[cfg(test)]
mod guide_hold {
    use core::time::Duration;
    use tehuti_core::{Level, State, step};
    use tehuti_protocol::{ButtonName, ButtonState, Event, parse_context};

    fn run(virtual_pad: bool, held_for: Duration) -> State {
        let context = parse_context(&format!(
            "app_id = 'game'\nbuttons = {{}}\n[pad]\npassthrough = true\nvirtual = {virtual_pad}"
        ))
        .unwrap();
        let mut state = step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("game".into()),
                title: String::new(),
            },
        )
        .0;
        for event in [
            Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Pressed,
            },
            Event::Tick { elapsed: held_for },
            Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Released,
            },
        ] {
            state = step(state, &event).0;
        }
        state
    }

    #[test]
    fn a_forwarded_hold_under_three_seconds_is_the_streams_alone() {
        let state = run(true, Duration::from_secs(2));
        assert!(state.menu().is_none());
        assert_eq!(state.level(), Level::App, "a hold is not a tap");
    }

    #[test]
    fn a_forwarded_hold_between_three_and_six_seconds_is_the_streams_alone() {
        let state = run(true, Duration::from_millis(5900));
        assert!(state.menu().is_none(), "no local hold action at 3 s");
    }

    #[test]
    fn a_forwarded_hold_of_six_seconds_fires_the_local_hold() {
        assert!(run(true, Duration::from_secs(6)).menu().is_some());
    }

    #[test]
    fn a_local_hold_fires_at_the_usual_time_when_nothing_is_forwarded() {
        assert!(run(false, Duration::from_secs(3)).menu().is_some());
        assert!(run(false, tehuti_core::GUIDE_TAP_WINDOW).menu().is_some());
    }
}
