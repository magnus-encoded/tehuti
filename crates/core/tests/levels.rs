//! A Guide tap switches LB and RB between the focused app and column focus.

#[cfg(test)]
mod levels {
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_core::{Level, State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, LayoutAction};

    const fn button(name: ButtonName, state: ButtonState) -> Event {
        Event::Button { name, state }
    }

    const fn tick(millis: u64) -> Event {
        Event::Tick {
            elapsed: Duration::from_millis(millis),
        }
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

    fn tap() -> Vec<Event> {
        vec![
            button(ButtonName::Guide, ButtonState::Pressed),
            button(ButtonName::Guide, ButtonState::Released),
        ]
    }

    fn focus(app_id: &str) -> Event {
        Event::Focus {
            app_id: Some(app_id.into()),
            title: String::new(),
        }
    }

    fn bar_level(state: &State) -> String {
        state
            .bar()
            .segments()
            .last()
            .map(|segment| segment.text.clone())
            .unwrap_or_default()
    }

    #[test]
    fn starts_in_app_and_shows_it() {
        let state = State::new();
        assert_eq!(state.level(), Level::InApp);
        assert_eq!(bar_level(&state), "App");
    }

    #[test]
    fn columns_level_moves_column_focus() {
        let (state, toggled) = run(State::new(), &tap());
        assert_eq!(toggled, Vec::new());
        assert_eq!(bar_level(&state), "Columns");
        let (_, actions) = run(
            state,
            &[
                button(ButtonName::LeftBumper, ButtonState::Pressed),
                button(ButtonName::LeftBumper, ButtonState::Released),
                button(ButtonName::RightBumper, ButtonState::Pressed),
                button(ButtonName::RightBumper, ButtonState::Released),
            ],
        );
        assert_eq!(
            actions,
            vec![
                Action::Layout {
                    action: LayoutAction::FocusColumnLeft
                },
                Action::Layout {
                    action: LayoutAction::FocusColumnRight
                },
            ]
        );
    }

    #[test]
    fn guide_at_499_ms_taps_and_at_500_ms_opens_the_menu() {
        for (millis, level, menu_open) in [(499, Level::Columns, false), (500, Level::InApp, true)] {
            let (state, actions) = run(
                State::new(),
                &[
                    button(ButtonName::Guide, ButtonState::Pressed),
                    tick(millis),
                    button(ButtonName::Guide, ButtonState::Released),
                ],
            );
            assert_eq!(state.level(), level);
            assert_eq!(state.menu().is_some(), menu_open);
            assert!(actions.is_empty());
        }
    }

    proptest! {
        #[test]
        fn each_tap_toggles(taps in 0usize..12, gap in 0u64..400) {
            let mut events = Vec::new();
            for _ in 0..taps {
                events.push(button(ButtonName::Guide, ButtonState::Pressed));
                events.push(tick(gap));
                events.push(button(ButtonName::Guide, ButtonState::Released));
            }
            let (state, _) = run(State::new(), &events);
            let expected = if taps % 2 == 0 { Level::InApp } else { Level::Columns };
            prop_assert_eq!(state.level(), expected);
        }

        #[test]
        fn escape_chord_quits_without_toggling(start_first in any::<bool>(), steps in 30u64..60) {
            let mut events = if start_first {
                vec![button(ButtonName::Start, ButtonState::Pressed), button(ButtonName::Guide, ButtonState::Pressed)]
            } else {
                vec![button(ButtonName::Guide, ButtonState::Pressed), button(ButtonName::Start, ButtonState::Pressed)]
            };
            events.extend((0..steps).map(|_| tick(100)));
            events.push(button(ButtonName::Guide, ButtonState::Released));
            events.push(button(ButtonName::Start, ButtonState::Released));
            let (state, actions) = run(State::new(), &events);
            prop_assert_eq!(actions, vec![Action::Quit]);
            prop_assert_eq!(state.level(), Level::InApp);
        }

        #[test]
        fn level_survives_focus_changes(apps in prop::collection::vec(prop::sample::select(vec!["atlas", "nova", "orbit"]), 0..10)) {
            let mut events = tap();
            events.extend(apps.iter().map(|app| focus(app)));
            let (state, _) = run(State::new(), &events);
            prop_assert_eq!(state.level(), Level::Columns);
            prop_assert_eq!(bar_level(&state), "Columns");
        }
    }
}
