//! Default layout phrases name whole units and destinations; relative steps stay on the pad.

#[cfg(test)]
mod layout_phrases {
    use core::num::NonZeroU8;

    use proptest::prelude::*;
    use tehuti_core::{State, layout, layout_phrases, step, ticker};
    use tehuti_protocol::{Action, Event, KeyName, LayoutAction, Text, parse_context};

    fn non_zero(value: u8) -> NonZeroU8 {
        NonZeroU8::new(value).unwrap()
    }

    fn speak(state: State, phrase: &str) -> (State, Vec<Action>) {
        step(
            state,
            &Event::Speech {
                text: Text::new(phrase.into()).unwrap(),
            },
        )
    }

    #[test]
    fn default_phrases_trigger_their_layout_actions_without_any_context() {
        let cases = [
            ("Workspace two.", LayoutAction::FocusWorkspace(non_zero(2))),
            ("workspace 2", LayoutAction::FocusWorkspace(non_zero(2))),
            ("WORKSPACE ONE!", LayoutAction::FocusWorkspace(non_zero(1))),
            (
                "Send to workspace two.",
                LayoutAction::MoveColumnToWorkspace(non_zero(2)),
            ),
            (
                "send to workspace 3",
                LayoutAction::MoveColumnToWorkspace(non_zero(3)),
            ),
            (
                "Send to workspace two and stay.",
                LayoutAction::MoveColumnToWorkspaceStay(non_zero(2)),
            ),
            (
                "Send and stay workspace two.",
                LayoutAction::MoveColumnToWorkspaceStay(non_zero(2)),
            ),
            (
                "Send and stay to workspace 4.",
                LayoutAction::MoveColumnToWorkspaceStay(non_zero(4)),
            ),
            (
                "Send and stay two.",
                LayoutAction::MoveColumnToWorkspaceStay(non_zero(2)),
            ),
            ("Full screen.", LayoutAction::ToggleFullscreen),
            ("fullscreen", LayoutAction::ToggleFullscreen),
            ("Full width.", LayoutAction::MaximizeColumn),
            ("Move left.", LayoutAction::MoveColumnLeft),
            ("Move right.", LayoutAction::MoveColumnRight),
        ];
        for (phrase, expected) in cases {
            let (next, actions) = speak(State::new(), phrase);
            assert_eq!(
                actions,
                vec![Action::Layout { action: expected }],
                "phrase {phrase:?}"
            );
            let bar_result = next.bar().segments().last().unwrap().text.clone();
            assert_eq!(
                layout_phrases::parse(&bar_result),
                Some(expected),
                "bar chip {bar_result:?} must name what voice accepts"
            );
        }
    }

    #[test]
    fn default_phrases_beat_unmatched_dictation_but_an_explicit_context_phrase_wins() {
        let context = parse_context(
            "app_id = 'terminal'\nunmatched_speech = 'type'\n[buttons]\n[speech]\n'Move left' = { key = 'h' }",
        )
        .unwrap();
        let (state, _) = step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("terminal".into()),
                title: String::new(),
            },
        );
        let (_, fullscreen) = speak(state.clone(), "Full screen.");
        assert_eq!(
            fullscreen,
            vec![Action::Layout {
                action: LayoutAction::ToggleFullscreen,
            }]
        );
        let (_, overridden) = speak(state, "Move left.");
        assert_eq!(
            overridden,
            vec![Action::Key {
                key: KeyName::H,
                mods: Vec::new(),
            }]
        );
    }

    #[test]
    fn relative_steps_and_invalid_workspace_numbers_never_match_voice() {
        for phrase in [
            "next workspace",
            "previous workspace",
            "workspace up",
            "workspace down",
            "workspace next",
            "workspace",
            "workspace zero",
            "workspace 0",
            "workspace 02",
            "send to workspace",
            "send to workspace 0",
            "send to workspace next",
            "send and stay",
            "send and stay 0",
            "column left",
            "column right",
            "window up",
            "window down",
            "wider",
            "narrower",
            "move up",
            "move down",
        ] {
            assert_eq!(
                layout_phrases::parse(phrase),
                None,
                "step or invalid phrase {phrase:?} must not match voice"
            );
        }
    }

    #[test]
    fn send_follows_and_send_and_stay_does_not() {
        assert!(
            layout::follows_moved_window(LayoutAction::MoveColumnToWorkspace(non_zero(2))),
            "send to workspace follows the window"
        );
        assert!(
            !layout::follows_moved_window(LayoutAction::MoveColumnToWorkspaceStay(non_zero(2))),
            "send and stay keeps focus on the current workspace"
        );
        assert_eq!(
            layout::command_for(LayoutAction::FocusWorkspace(non_zero(2))),
            layout::Command::FocusWorkspaceAt(1)
        );
        assert_eq!(
            layout::command_for(LayoutAction::MoveColumnToWorkspace(non_zero(3))),
            layout::Command::MoveColumnToWorkspaceAt(2)
        );
        assert_eq!(
            layout::command_for(LayoutAction::MoveColumnToWorkspaceStay(non_zero(3))),
            layout::Command::MoveColumnToWorkspaceAt(2)
        );
    }

    const NUMBER_WORDS: [&str; 20] = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];

    proptest! {
        #[test]
        fn every_unit_label_parses_back_to_the_same_action(workspace in 1_u8..=20) {
            let number = non_zero(workspace);
            let units = [
                LayoutAction::FocusWorkspace(number),
                LayoutAction::MoveColumnToWorkspace(number),
                LayoutAction::MoveColumnToWorkspaceStay(number),
                LayoutAction::ToggleFullscreen,
                LayoutAction::MaximizeColumn,
                LayoutAction::MoveColumnLeft,
                LayoutAction::MoveColumnRight,
            ];
            for action in units {
                let shown = ticker::layout_text(action);
                prop_assert_eq!(layout_phrases::parse(&shown), Some(action), "label {:?}", shown);
            }
        }

        #[test]
        fn every_workspace_word_and_digit_maps_to_its_zero_based_command(index in 0_usize..20) {
            let word = NUMBER_WORDS[index];
            let one_based = u8::try_from(index.saturating_add(1)).unwrap();
            let digits = one_based.to_string();
            for spoken in [word, digits.as_str()] {
                let focus = layout_phrases::parse(&format!("Workspace {spoken}.")).unwrap();
                prop_assert_eq!(layout::command_for(focus), layout::Command::FocusWorkspaceAt(index));
                let send = layout_phrases::parse(&format!("Send to workspace {spoken}.")).unwrap();
                prop_assert_eq!(layout::command_for(send), layout::Command::MoveColumnToWorkspaceAt(index));
                prop_assert!(layout::follows_moved_window(send));
                let stay = layout_phrases::parse(&format!("Send to workspace {spoken} and stay.")).unwrap();
                prop_assert_eq!(layout::command_for(stay), layout::Command::MoveColumnToWorkspaceAt(index));
                prop_assert!(!layout::follows_moved_window(stay));
            }
        }
    }
}
