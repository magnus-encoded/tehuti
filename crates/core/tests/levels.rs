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

    fn bar_text(state: &State) -> String {
        state
            .bar()
            .segments()
            .last()
            .map(|segment| segment.text.clone())
            .unwrap_or_default()
    }

    #[test]
    fn starts_at_app_level_without_a_level_chip() {
        let state = State::new();
        assert_eq!(state.level(), Level::App);
        assert_eq!(bar_text(&state), "Desktop");
    }

    #[test]
    fn neither_level_adds_any_label_to_the_bar() {
        use tehuti_core::bar::{Segment, Tone};
        let (app, _) = run(State::new(), &[focus("org.example.Editor")]);
        let (columns, _) = run(app.clone(), &tap());
        for state in [app, columns] {
            for windows in [vec![], vec![("Notes".into(), true)]] {
                let bar = state.bar().clone().with_windows(windows.clone());
                let text = if state.level() == Level::Columns && !windows.is_empty() {
                    "Notes"
                } else {
                    "org.example.Editor"
                };
                assert_eq!(
                    bar.segments(),
                    vec![Segment {
                        text: text.into(),
                        tone: Tone::Context
                    }]
                );
            }
        }
    }

    #[test]
    fn columns_list_titles_in_order_and_mark_only_the_focused_window() {
        use tehuti_core::bar::{Segment, Tone};
        let (state, _) = run(State::new(), &tap());
        let bar = state.bar().clone().with_windows(vec![
            ("Notes".into(), false),
            ("Terminal".into(), true),
            ("Browser".into(), false),
        ]);
        assert_eq!(
            bar.segments(),
            vec![
                Segment {
                    text: "Notes".into(),
                    tone: Tone::Unfocused
                },
                Segment {
                    text: "Terminal".into(),
                    tone: Tone::Context
                },
                Segment {
                    text: "Browser".into(),
                    tone: Tone::Unfocused
                },
            ]
        );
        let app = State::new()
            .bar()
            .clone()
            .with_windows(vec![("Notes".into(), true)]);
        assert_eq!(app.segments().len(), 1);
        assert_eq!(app.segments()[0].text, "Desktop");
    }

    #[test]
    fn columns_level_moves_column_focus() {
        let (state, toggled) = run(State::new(), &tap());
        assert_eq!(toggled, Vec::new());
        assert_eq!(bar_text(&state), "Desktop");
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
        for (millis, level, menu_open) in [(499, Level::Columns, false), (500, Level::App, true)] {
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
            assert_eq!(actions, Vec::<Action>::new());
        }
    }

    #[test]
    fn guide_vertical_dpad_focuses_workspaces_and_consumes_mapped_buttons() {
        for (name, action) in [
            (ButtonName::DpadUp, LayoutAction::FocusWorkspaceUp),
            (ButtonName::DpadDown, LayoutAction::FocusWorkspaceDown),
        ] {
            let context = tehuti_protocol::parse_context(
                "app_id = '*'\n[buttons]\ndpad_up = { key = 'a' }\ndpad_down = { key = 'b' }",
            )
            .unwrap();
            let (state, _) = step(
                State::with_contexts(vec![context]),
                &button(ButtonName::Guide, ButtonState::Pressed),
            );
            let (state, actions) = step(state, &button(name, ButtonState::Pressed));
            assert_eq!(actions, vec![Action::Layout { action }]);
            let (_, released) = step(state, &button(name, ButtonState::Released));
            assert_eq!(released, Vec::<Action>::new());
        }
    }

    #[test]
    fn workspace_chord_cancels_the_guide_menu_and_level_tap() {
        for name in [ButtonName::DpadUp, ButtonName::DpadDown] {
            for delay in [0, 500, 3000] {
                let (state, _) = run(
                    State::new(),
                    &[
                        button(ButtonName::Guide, ButtonState::Pressed),
                        button(name, ButtonState::Pressed),
                        button(name, ButtonState::Released),
                        tick(delay),
                    ],
                );
                assert!(state.menu().is_none());
                let (state, actions) = step(state, &button(ButtonName::Guide, ButtonState::Released));
                assert!(state.menu().is_none());
                assert_eq!(state.level(), Level::App);
                assert_eq!(actions, Vec::<Action>::new());
            }
        }
    }

    #[test]
    fn workspace_chord_preserves_the_three_second_quit() {
        let (state, actions) = run(
            State::new(),
            &[
                button(ButtonName::Guide, ButtonState::Pressed),
                button(ButtonName::Start, ButtonState::Pressed),
                tick(1500),
                button(ButtonName::DpadDown, ButtonState::Pressed),
                button(ButtonName::DpadDown, ButtonState::Released),
                tick(1499),
            ],
        );
        assert_eq!(
            actions,
            vec![Action::Layout {
                action: LayoutAction::FocusWorkspaceDown
            }]
        );
        let (state, quit) = step(state, &tick(1));
        assert_eq!(quit, vec![Action::Quit]);
        assert!(state.menu().is_none());
    }

    #[test]
    fn guide_horizontal_dpad_has_no_workspace_binding() {
        let (_, actions) = run(
            State::new(),
            &[
                button(ButtonName::Guide, ButtonState::Pressed),
                button(ButtonName::DpadLeft, ButtonState::Pressed),
                button(ButtonName::DpadLeft, ButtonState::Released),
                button(ButtonName::DpadRight, ButtonState::Pressed),
                button(ButtonName::DpadRight, ButtonState::Released),
            ],
        );
        assert_eq!(actions, Vec::<Action>::new());
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
            let expected = if taps % 2 == 0 { Level::App } else { Level::Columns };
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
            prop_assert_eq!(state.level(), Level::App);
        }

        #[test]
        fn level_survives_focus_changes(apps in prop::collection::vec(prop::sample::select(vec!["atlas", "nova", "orbit"]), 0..10)) {
            let mut events = tap();
            events.extend(apps.iter().map(|app| focus(app)));
            let (state, _) = run(State::new(), &events);
            prop_assert_eq!(state.level(), Level::Columns);
            prop_assert_eq!(bar_text(&state), apps.last().copied().unwrap_or("Desktop"));
        }
    }
}
