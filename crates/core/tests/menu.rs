//! Properties of the system menu over generated input series.

#[cfg(test)]
mod menu {
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_core::{
        State,
        menu::{Command, ITEMS, Item},
        step, step_with_plugin,
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event};

    fn button(name: ButtonName, state: ButtonState) -> Event {
        Event::Button { name, state }
    }

    fn press(name: ButtonName) -> Event {
        button(name, ButtonState::Pressed)
    }

    fn opened() -> State {
        let (state, _) = step(State::new(), &press(ButtonName::Guide));
        step(
            state,
            &Event::Tick {
                elapsed: Duration::from_secs(3),
            },
        )
        .0
    }

    fn navigation() -> impl Strategy<Value = Vec<ButtonName>> {
        prop::collection::vec(
            prop_oneof![Just(ButtonName::DpadUp), Just(ButtonName::DpadDown)],
            0..60,
        )
    }

    fn moved(presses: &[ButtonName]) -> (State, usize) {
        let mut state = opened();
        let mut index = 0usize;
        for name in presses {
            state = step(state, &press(*name)).0;
            index = if *name == ButtonName::DpadDown {
                index.saturating_add(1).checked_rem(ITEMS.len()).unwrap_or(0)
            } else {
                index.checked_sub(1).unwrap_or(ITEMS.len().saturating_sub(1))
            };
        }
        (state, index)
    }

    fn selected(state: &State) -> Item {
        state.menu().expect("menu open").selected()
    }

    #[test]
    fn items_are_the_owner_specified_set_in_order() {
        let labels: Vec<_> = ITEMS.iter().map(|item| item.label()).collect();
        assert_eq!(
            labels,
            [
                "CLOSE APP",
                "SCREENSHOT",
                "LOCK",
                "LOG OUT",
                "RESTART TEHUTI",
                "REBOOT",
                "POWER OFF",
                "CANCEL"
            ]
        );
        assert_eq!(selected(&opened()), Item::CloseApp);
    }

    proptest! {
        #[test]
        fn dpad_moves_the_highlight_and_wraps(presses in navigation()) {
            let (state, index) = moved(&presses);
            prop_assert_eq!(selected(&state), ITEMS[index]);
            let text = state.menu().unwrap().text();
            let marked: Vec<_> = text.lines().filter(|line| line.starts_with("> ")).collect();
            prop_assert_eq!(marked, vec![format!("> {}", ITEMS[index].label())]);
        }

        #[test]
        fn south_chooses_the_highlighted_item_and_closes(presses in navigation()) {
            let (state, index) = moved(&presses);
            let (mut state, actions) = step(state, &press(ButtonName::South));
            prop_assert!(state.menu().is_none());
            let commands = state.take_commands();
            let (want_actions, want_commands) = match ITEMS[index] {
                Item::CloseApp => (vec![Action::Close], vec![]),
                Item::Screenshot => (vec![], vec![Command::Screenshot]),
                Item::Lock => (vec![], vec![Command::Lock]),
                Item::LogOut => (vec![], vec![Command::LogOut]),
                Item::RestartTehuti => (vec![], vec![Command::Restart]),
                Item::Reboot => (vec![], vec![Command::Reboot]),
                Item::PowerOff => (vec![], vec![Command::PowerOff]),
                Item::Cancel => (vec![], vec![]),
            };
            prop_assert_eq!(actions, want_actions, "South never sends Enter while the menu is open");
            prop_assert_eq!(commands, want_commands);
        }

        #[test]
        fn east_and_guide_close_without_effect(presses in navigation(), closer in prop_oneof![Just(ButtonName::East), Just(ButtonName::Guide), Just(ButtonName::GuideTap)]) {
            let (state, _) = moved(&presses);
            let (mut state, actions) = step(state, &press(closer));
            prop_assert!(state.menu().is_none());
            prop_assert!(actions.is_empty());
            prop_assert!(state.take_commands().is_empty());
        }

        #[test]
        fn while_open_every_other_input_is_consumed_and_changes_nothing(
            events in prop::collection::vec(any::<Event>().prop_filter("not a menu control", |event| !matches!(event,
                Event::Button { name: ButtonName::DpadUp | ButtonName::DpadDown | ButtonName::South | ButtonName::East | ButtonName::Guide | ButtonName::GuideTap, state: ButtonState::Pressed })), 0..80),
            plugin_active in any::<bool>(),
        ) {
            let mut state = opened();
            for event in events {
                let before = state.menu().cloned();
                let focus = matches!(event, Event::Focus { .. });
                let (next, actions, sent) = step_with_plugin(state, &event, plugin_active);
                state = next;
                prop_assert!(!sent, "no plugin sees input while the menu is open");
                prop_assert!(actions.is_empty());
                prop_assert!(state.take_commands().is_empty());
                prop_assert_eq!(state.menu().cloned(), before.clone(), "focus {}", focus);
            }
        }
    }

    #[test]
    fn menu_text_marks_one_row_per_item() {
        let text = opened().menu().unwrap().text();
        assert_eq!(text.lines().count(), ITEMS.len() + 1);
        assert_eq!(text.lines().next(), Some("SYSTEM MENU"));
    }

    #[test]
    fn the_menu_does_not_reopen_while_guide_stays_held() {
        let state = opened();
        let (state, _) = step(state, &press(ButtonName::East));
        let (state, _) = step(
            state,
            &Event::Tick {
                elapsed: Duration::from_secs(9),
            },
        );
        assert!(state.menu().is_none(), "closing needs a fresh press and hold");
    }
}
