//! Neovim context: modal sequences, leader phrases, stick scrolling, and ticker visibility.

#[cfg(test)]
mod neovim_context {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        Level, State, step,
        stick_actions::StickActions,
        ticker::{self, PadFamily},
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Modifier, Text, parse_context};

    const NEOVIM_SOURCE: &str = include_str!("../../../examples/neovim/context.toml");

    fn key(name: KeyName) -> Action {
        Action::Key {
            key: name,
            mods: Vec::new(),
        }
    }

    fn chord(name: KeyName, mods: Vec<Modifier>) -> Action {
        Action::Key { key: name, mods }
    }

    fn text(value: &str) -> Action {
        Action::Text {
            text: Text::new(value.into()).unwrap(),
        }
    }

    fn focused_neovim() -> State {
        let context = parse_context(NEOVIM_SOURCE).unwrap();
        step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("nvim".into()),
                title: String::new(),
            },
        )
        .0
    }

    #[test]
    fn voice_destinations_return_to_normal_mode_and_emit_expected_sequences() {
        let state = focused_neovim();
        let cases: [(&str, Vec<Action>); 10] = [
            (
                "Save.",
                vec![key(KeyName::Escape), text(":w"), key(KeyName::Enter)],
            ),
            (
                "Save and quit.",
                vec![key(KeyName::Escape), text(":wq"), key(KeyName::Enter)],
            ),
            (
                "Find file.",
                vec![
                    key(KeyName::Escape),
                    key(KeyName::Space),
                    key(KeyName::F),
                    key(KeyName::F),
                ],
            ),
            (
                "Go to line 40.",
                vec![key(KeyName::Escape), text(":40"), key(KeyName::Enter)],
            ),
            (
                "Go to line forty.",
                vec![key(KeyName::Escape), text(":40"), key(KeyName::Enter)],
            ),
            (
                "Go to line one hundred and twenty five.",
                vec![key(KeyName::Escape), text(":125"), key(KeyName::Enter)],
            ),
            (
                "Split right.",
                vec![key(KeyName::Escape), text(":vsplit"), key(KeyName::Enter)],
            ),
            (
                "Split below.",
                vec![key(KeyName::Escape), text(":split"), key(KeyName::Enter)],
            ),
            ("Normal mode.", vec![key(KeyName::Escape)]),
            ("Insert mode.", vec![key(KeyName::Escape), key(KeyName::I)]),
        ];
        for (spoken, expected) in cases {
            let (_, actions) = step(
                state.clone(),
                &Event::Speech {
                    text: Text::new(spoken.into()).unwrap(),
                },
            );
            assert_eq!(actions, expected, "phrase {spoken:?}");
        }
    }

    #[test]
    fn go_to_line_rejects_non_numeric_tails() {
        let context = parse_context(NEOVIM_SOURCE).unwrap();
        for invalid in ["Go to line banana.", "Go to line 40; !sh", "Go to line."] {
            assert_eq!(
                context.speech_actions(invalid),
                None,
                "invalid line target {invalid:?} must not expand"
            );
        }
    }

    fn split_move(direction: KeyName) -> Vec<Action> {
        vec![
            key(KeyName::Escape),
            chord(KeyName::W, vec![Modifier::Ctrl]),
            key(direction),
        ]
    }

    #[test]
    fn controller_buttons_step_buffers_splits_and_search_matches() {
        let state = focused_neovim();
        let cases: [(ButtonName, Vec<Action>); 9] = [
            (
                ButtonName::LeftBumper,
                vec![key(KeyName::Escape), text(":bprevious"), key(KeyName::Enter)],
            ),
            (
                ButtonName::RightBumper,
                vec![key(KeyName::Escape), text(":bnext"), key(KeyName::Enter)],
            ),
            (ButtonName::DpadLeft, split_move(KeyName::H)),
            (ButtonName::DpadDown, split_move(KeyName::J)),
            (ButtonName::DpadUp, split_move(KeyName::K)),
            (ButtonName::DpadRight, split_move(KeyName::L)),
            (ButtonName::West, vec![key(KeyName::Escape), key(KeyName::N)]),
            (
                ButtonName::LeftTrigger,
                vec![key(KeyName::Escape), chord(KeyName::N, vec![Modifier::Shift])],
            ),
            (
                ButtonName::RightTrigger,
                vec![key(KeyName::Escape), key(KeyName::U)],
            ),
        ];
        for (button, expected) in cases {
            let (_, actions) = step(
                state.clone(),
                &Event::Button {
                    name: button,
                    state: ButtonState::Pressed,
                },
            );
            assert_eq!(actions, expected, "button {button:?}");
        }
    }

    #[test]
    fn sticks_scroll_after_returning_to_normal_mode() {
        let context = parse_context(NEOVIM_SOURCE).unwrap();
        let frame = Duration::from_millis(16);
        for (left, right, expected) in [
            (
                (0, -1000),
                (0, 0),
                vec![key(KeyName::Escape), chord(KeyName::U, vec![Modifier::Ctrl])],
            ),
            (
                (0, 1000),
                (0, 0),
                vec![key(KeyName::Escape), chord(KeyName::D, vec![Modifier::Ctrl])],
            ),
            (
                (0, 0),
                (0, -1000),
                vec![key(KeyName::Escape), chord(KeyName::Y, vec![Modifier::Ctrl])],
            ),
            (
                (0, 0),
                (0, 1000),
                vec![key(KeyName::Escape), chord(KeyName::E, vec![Modifier::Ctrl])],
            ),
        ] {
            let mut sticks = StickActions::default();
            let actions = sticks.tick(&context, frame, left, right);
            assert_eq!(actions, expected, "stick sample ({left:?}, {right:?})");
        }
    }

    #[test]
    fn every_neovim_command_starts_with_escape_except_confirm() {
        let context = parse_context(NEOVIM_SOURCE).unwrap();
        for button in context.mapped_buttons() {
            if button == ButtonName::South {
                continue;
            }
            let actions = context.actions_for(button).unwrap();
            assert_eq!(
                actions.first(),
                Some(&key(KeyName::Escape)),
                "button {button:?} must return to Normal mode first"
            );
        }
        for mapping in context.stick_mappings() {
            assert_eq!(
                mapping.actions.first(),
                Some(&key(KeyName::Escape)),
                "stick {:?} {:?} must return to Normal mode first",
                mapping.stick,
                mapping.direction
            );
        }
        for mapping in context.speech_mappings() {
            assert_eq!(
                mapping.actions.first(),
                Some(&key(KeyName::Escape)),
                "phrase {:?} must return to Normal mode first",
                mapping.phrase
            );
        }
    }

    #[test]
    fn mapping_ticker_lists_every_phrase_and_pad_mapping_without_control_chars() {
        let context = parse_context(NEOVIM_SOURCE).unwrap();
        let contexts = vec![context.clone()];
        assert!(
            ticker::enabled(&contexts, Some("nvim")),
            "neovim enables the mapping ticker"
        );
        let entries = ticker::entries(&contexts, Some("nvim"), PadFamily::Letters, Level::App);
        let rendered: Vec<String> = entries.iter().map(|entry| entry.text(|_| true)).collect();
        for line in &rendered {
            assert!(
                !line.chars().any(char::is_control),
                "ticker line must not contain control characters: {line:?}"
            );
        }
        for required_button in [
            "A -> Confirm",
            "B -> Normal mode",
            "X -> Next match",
            "LB -> Prev buffer",
            "RB -> Next buffer",
            "LT -> Prev match",
            "RT -> Undo",
            "Left -> Split left",
            "Down -> Split down",
            "Up -> Split up",
            "Right -> Split right",
        ] {
            assert!(
                rendered.contains(&required_button.to_owned()),
                "missing pad entry {required_button:?} in {rendered:?}"
            );
        }
        for required_stick in ["L\u{2191} ->", "L\u{2193} ->", "R\u{2191} ->", "R\u{2193} ->"] {
            assert!(
                rendered.iter().any(|line| line.starts_with(required_stick)),
                "missing stick entry starting with {required_stick:?} in {rendered:?}"
            );
        }
        for mapping in context.speech_mappings() {
            let quoted = format!("\"{}\"", mapping.phrase);
            assert!(
                rendered.iter().any(|line| line.contains(&quoted)),
                "every voice unit must show its spoken name {quoted:?} in {rendered:?}"
            );
        }
    }

    #[test]
    fn invalid_sequences_and_number_templates_are_rejected_at_the_boundary() {
        for bad in [
            "[buttons]\neast = { sequence = [] }",
            "[buttons]\neast = { sequence = [{ key = 'escape', hold = true }] }",
            "[buttons]\neast = { sequence = [{ sequence = [{ key = 'escape' }] }] }",
            "[buttons]\neast = { sequence = [{ key = 'bogus_key' }] }",
            "[speech]\n'Go to line <number>' = { key = 'enter' }",
            "[speech]\n'Go to line <number>' = { sequence = [{ key = 'escape' }, { text = ':1' }] }",
            "[speech]\n'<number>' = { text = ':{number}' }",
        ] {
            let source = format!("app_id = 'nvim'\n{bad}\n");
            let error = parse_context(&source).unwrap_err();
            if bad.contains("bogus_key") {
                assert!(
                    error.contains("bogus_key"),
                    "unknown key inside sequence should name the key: {error}"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn go_to_line_expands_every_numeric_line_target(line in 1..100_000_u64) {
            let context = parse_context(NEOVIM_SOURCE).unwrap();
            let spoken = format!("Go to line {line}.");
            prop_assert_eq!(
                context.speech_actions(&spoken),
                Some(vec![
                    key(KeyName::Escape),
                    text(&format!(":{line}")),
                    key(KeyName::Enter),
                ])
            );
        }

        #[test]
        fn sequences_preserve_step_order_through_core_step(
            keys in prop::collection::vec(
                prop::sample::select(vec![
                    ("escape", KeyName::Escape),
                    ("space", KeyName::Space),
                    ("enter", KeyName::Enter),
                    ("h", KeyName::H),
                    ("j", KeyName::J),
                    ("k", KeyName::K),
                    ("l", KeyName::L),
                ]),
                1..8,
            ),
        ) {
            let steps: Vec<String> = keys
                .iter()
                .map(|(name, _)| format!("{{ key = '{name}' }}"))
                .collect();
            let source = format!(
                "app_id = 'editor'\n[buttons]\neast = {{ sequence = [{}] }}\n",
                steps.join(", ")
            );
            let context = parse_context(&source).unwrap();
            let (state, _) = step(
                State::with_contexts(vec![context]),
                &Event::Focus {
                    app_id: Some("editor".into()),
                    title: String::new(),
                },
            );
            let (_, actions) = step(
                state,
                &Event::Button {
                    name: ButtonName::East,
                    state: ButtonState::Pressed,
                },
            );
            let expected: Vec<Action> = keys.into_iter().map(|(_, name)| key(name)).collect();
            prop_assert_eq!(actions, expected);
        }
    }
}
