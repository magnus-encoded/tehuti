//! `[pad] guide` chooses who receives Guide in a passthrough context with a virtual pad.

#[cfg(test)]
mod guide_mode {
    use core::time::Duration;

    use tehuti_core::{Level, State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, GuideMode, parse_context};

    fn game(guide: &str) -> State {
        let source =
            format!("app_id = 'game'\n[buttons]\n[pad]\npassthrough = true\nvirtual = true\n{guide}");
        focus(
            State::with_contexts(vec![parse_context(&source).unwrap()]),
            "game",
        )
        .0
    }

    fn focus(state: State, app: &str) -> (State, Vec<Action>) {
        step(
            state,
            &Event::Focus {
                app_id: Some(app.into()),
                title: String::new(),
            },
        )
    }

    fn button(name: ButtonName, state: ButtonState) -> Event {
        Event::Button { name, state }
    }

    fn run(mut state: State, events: &[Event]) -> (State, Vec<Action>) {
        let mut actions = Vec::new();
        for event in events {
            let (next, emitted) = step(state, event);
            state = next;
            actions.extend(emitted);
        }
        (state, actions)
    }

    #[test]
    fn the_mode_defaults_to_tehuti_and_follows_the_context() {
        assert_eq!(game("").guide_mode(), GuideMode::Tehuti);
        assert_eq!(game("guide = 'both'").guide_mode(), GuideMode::Both);
        assert_eq!(game("guide = 'app'").guide_mode(), GuideMode::App);
    }

    #[test]
    fn a_tap_changes_the_level_in_tehuti_and_both_modes_but_not_in_app_mode() {
        let tap = [
            button(ButtonName::Guide, ButtonState::Pressed),
            button(ButtonName::Guide, ButtonState::Released),
        ];
        assert_eq!(run(game(""), &tap).0.level(), Level::Columns);
        assert_eq!(run(game("guide = 'both'"), &tap).0.level(), Level::Columns);
        assert_eq!(run(game("guide = 'app'"), &tap).0.level(), Level::App);
    }

    #[test]
    fn guide_with_the_dpad_does_nothing_in_app_mode_but_moves_in_tehuti_mode() {
        let chord = [
            button(ButtonName::Guide, ButtonState::Pressed),
            button(ButtonName::DpadDown, ButtonState::Pressed),
            button(ButtonName::DpadDown, ButtonState::Released),
            button(ButtonName::Guide, ButtonState::Released),
        ];
        let (state, actions) = run(game("guide = 'app'"), &chord);
        assert!(actions.is_empty(), "{actions:?}");
        assert_eq!(state.level(), Level::App);
        let (_, actions) = run(game(""), &chord);
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, Action::Layout { .. }))
        );
    }

    #[test]
    fn the_escape_chord_quits_in_app_mode() {
        let mut events = vec![
            button(ButtonName::Guide, ButtonState::Pressed),
            button(ButtonName::Start, ButtonState::Pressed),
        ];
        events.extend((0..30).map(|_| Event::Tick {
            elapsed: Duration::from_millis(100),
        }));
        let (_, actions) = run(game("guide = 'app'"), &events);
        assert!(actions.contains(&Action::Quit));
    }

    #[test]
    fn app_mode_shows_a_notice_once_per_focus_naming_the_escape_chord() {
        let (state, actions) = focus(
            State::with_contexts(vec![
                parse_context(
                    "app_id = 'game'\n[buttons]\n[pad]\npassthrough = true\nvirtual = true\nguide = 'app'",
                )
                .unwrap(),
            ]),
            "game",
        );
        let notices: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Notice { text } => Some(text.as_str().to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("GUIDE+START"), "{notices:?}");
        let (_, again) = focus(state, "game");
        assert!(
            again
                .iter()
                .all(|action| !matches!(action, Action::Notice { .. }))
        );
    }

    #[test]
    fn other_modes_show_no_notice() {
        for guide in ["", "guide = 'both'"] {
            let source =
                format!("app_id = 'game'\n[buttons]\n[pad]\npassthrough = true\nvirtual = true\n{guide}");
            let (_, actions) = focus(
                State::with_contexts(vec![parse_context(&source).unwrap()]),
                "game",
            );
            assert!(
                actions
                    .iter()
                    .all(|action| !matches!(action, Action::Notice { .. }))
            );
        }
    }
}
