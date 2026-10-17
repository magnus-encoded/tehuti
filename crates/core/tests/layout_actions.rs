//! A pad button or phrase mapped to a tiling action changes the layout exactly as
//! the layout command it names.

#[cfg(test)]
mod layout_actions {
    use core::fmt::Write as _;

    use proptest::prelude::*;
    use tehuti_core::{
        State,
        layout::{
            self, Command, Layout, LayoutConfig, OutputGeometry, OutputId, Rect, WindowId, WindowRules,
        },
        step,
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, LayoutAction, Text, parse_context};

    /// The meaning of each action, written out independently of `command_for`.
    const TABLE: [(LayoutAction, &str, Command); 13] = [
        (
            LayoutAction::FocusColumnLeft,
            "focus-column-left",
            Command::FocusColumnLeft,
        ),
        (
            LayoutAction::FocusColumnRight,
            "focus-column-right",
            Command::FocusColumnRight,
        ),
        (
            LayoutAction::FocusWindowUp,
            "focus-window-up",
            Command::FocusWindowUp,
        ),
        (
            LayoutAction::FocusWindowDown,
            "focus-window-down",
            Command::FocusWindowDown,
        ),
        (
            LayoutAction::MoveColumnLeft,
            "move-column-left",
            Command::MoveColumnLeft,
        ),
        (
            LayoutAction::MoveColumnRight,
            "move-column-right",
            Command::MoveColumnRight,
        ),
        (
            LayoutAction::SwitchPresetColumnWidth,
            "switch-preset-column-width",
            Command::SwitchPresetColumnWidth,
        ),
        (
            LayoutAction::FocusWorkspaceDown,
            "focus-workspace-down",
            Command::FocusWorkspaceDown,
        ),
        (
            LayoutAction::FocusWorkspaceUp,
            "focus-workspace-up",
            Command::FocusWorkspaceUp,
        ),
        (
            LayoutAction::MoveColumnToWorkspaceDown,
            "move-column-to-workspace-down",
            Command::MoveColumnToWorkspaceDown,
        ),
        (
            LayoutAction::MoveColumnToWorkspaceUp,
            "move-column-to-workspace-up",
            Command::MoveColumnToWorkspaceUp,
        ),
        (
            LayoutAction::ToggleFullscreen,
            "toggle-fullscreen",
            Command::ToggleFullscreen,
        ),
        (
            LayoutAction::MaximizeColumn,
            "maximize-column",
            Command::MaximizeColumn,
        ),
    ];

    fn context() -> tehuti_protocol::Context {
        let mut source = String::from("app_id = 'test'\n[buttons]\n");
        source
            .push_str("east = { layout = 'focus-column-left' }\nwest = { layout = 'focus-column-right' }\n");
        source.push_str("[speech]\n");
        for (_, name, _) in TABLE {
            writeln!(source, "{:?} = {{ layout = '{name}' }}", name.replace('-', " ")).unwrap();
        }
        parse_context(&source).unwrap()
    }

    fn layout(windows: u64) -> Layout {
        let mut layout = Layout::try_new(LayoutConfig::default()).unwrap();
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        layout
            .add_output(OutputId(0), OutputGeometry { area, usable: area }, None)
            .unwrap();
        for window in 0..windows {
            layout
                .add_window(WindowId(window), WindowRules::default())
                .unwrap();
        }
        layout
    }

    #[test]
    fn every_action_name_parses_for_buttons_and_speech() {
        let context = context();
        assert_eq!(
            context.action_for(ButtonName::East),
            Some(Action::Layout {
                action: LayoutAction::FocusColumnLeft
            })
        );
        assert_eq!(context.key_for(ButtonName::West), None);
        for (action, name, _) in TABLE {
            assert_eq!(
                context.speech_phrase_action(&name.replace('-', " ")),
                Some(Action::Layout { action }),
                "{name}"
            );
        }
    }

    #[test]
    fn layout_actions_are_reserved_for_the_core() {
        for (action, _, _) in TABLE {
            assert!(
                tehuti_core::plugin::validate_actions(
                    vec![Action::Layout { action }],
                    &[
                        tehuti_protocol::Permission::Key,
                        tehuti_protocol::Permission::Text,
                        tehuti_protocol::Permission::Quit
                    ],
                )
                .is_err()
            );
        }
    }

    proptest! {
        #[test]
        fn routed_actions_match_direct_layout_commands(
            windows in 1_u64..6,
            script in prop::collection::vec((0..TABLE.len(), any::<bool>()), 0..24),
        ) {
            let (state, _) = step(
                State::with_contexts(vec![context()]),
                &Event::Focus { app_id: Some("test".into()), title: String::new() },
            );
            let mut routed = layout(windows);
            let mut direct = layout(windows);
            for (index, by_button) in script {
                let (action, name, command) = TABLE[index];
                let event = match (by_button, action) {
                    (true, LayoutAction::FocusColumnLeft) => Event::Button { name: ButtonName::East, state: ButtonState::Pressed },
                    (true, LayoutAction::FocusColumnRight) => Event::Button { name: ButtonName::West, state: ButtonState::Pressed },
                    _ => Event::Speech { text: Text::new(name.replace('-', " ")).unwrap() },
                };
                let (_, actions) = step(state.clone(), &event);
                prop_assert_eq!(&actions, &vec![Action::Layout { action }]);
                for routed_action in actions {
                    if let Action::Layout { action: mapped } = routed_action {
                        routed.apply(layout::command_for(mapped));
                    }
                }
                direct.apply(command);
                prop_assert_eq!(routed.snapshot(), direct.snapshot());
            }
        }
    }
}
