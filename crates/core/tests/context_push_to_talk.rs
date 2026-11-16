//! A focused context's `[pad] push_to_talk` replaces the global push-to-talk buttons.

#[cfg(test)]
mod context_push_to_talk {
    use tehuti_core::{State, step};
    use tehuti_protocol::{ButtonName, Event, parse_context};

    const GLOBAL: &str = "app_id = '*'\npush_to_talk = ['start']\n[buttons]";
    const GAME: &str =
        "app_id = 'game'\n[buttons]\n[pad]\npassthrough = true\npush_to_talk = ['select', 'left_bumper']";
    const EDITOR: &str = "app_id = 'editor'\n[buttons]";

    fn focused(app: &str) -> State {
        let contexts = [GLOBAL, GAME, EDITOR]
            .into_iter()
            .map(|source| parse_context(source).unwrap())
            .collect();
        step(
            State::with_contexts(contexts),
            &Event::Focus {
                app_id: Some(app.into()),
                title: String::new(),
            },
        )
        .0
    }

    #[test]
    fn the_context_chord_replaces_the_global_buttons() {
        let state = focused("game");
        assert!(!state.push_to_talk(ButtonName::Start));
        assert!(state.push_to_talk(ButtonName::Select));
        assert!(state.push_to_talk(ButtonName::LeftBumper));
        assert_eq!(
            state.passthrough_push_to_talk(),
            [ButtonName::Select, ButtonName::LeftBumper]
        );
    }

    #[test]
    fn a_context_without_its_own_list_restores_the_global_buttons() {
        let state = step(
            focused("game"),
            &Event::Focus {
                app_id: Some("editor".into()),
                title: String::new(),
            },
        )
        .0;
        assert!(state.push_to_talk(ButtonName::Start));
        assert!(!state.push_to_talk(ButtonName::Select));
    }
}
