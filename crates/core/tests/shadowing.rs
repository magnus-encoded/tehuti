#[cfg(test)]
mod shadowing {
    use core::fmt::Write as _;

    use proptest::prelude::*;
    use tehuti_core::{
        State,
        shadowing::{ButtonOwner, Shadowing, shadowed},
        step,
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Context, Event, Text, parse_context};

    const BUTTONS: [(ButtonName, &str); 16] = [
        (ButtonName::South, "south"),
        (ButtonName::East, "east"),
        (ButtonName::North, "north"),
        (ButtonName::West, "west"),
        (ButtonName::LeftBumper, "left_bumper"),
        (ButtonName::RightBumper, "right_bumper"),
        (ButtonName::Select, "select"),
        (ButtonName::Record, "record"),
        (ButtonName::Start, "start"),
        (ButtonName::Guide, "guide"),
        (ButtonName::LeftStick, "left_stick"),
        (ButtonName::RightStick, "right_stick"),
        (ButtonName::DpadUp, "dpad_up"),
        (ButtonName::DpadDown, "dpad_down"),
        (ButtonName::DpadLeft, "dpad_left"),
        (ButtonName::DpadRight, "dpad_right"),
    ];

    /// Every mapping types text naming its context and button, so a press shows which mapping fired.
    fn marker(context: usize, button: &str) -> String {
        format!("context {context} {button}")
    }

    fn context_source(
        index: usize,
        app_id: &str,
        buttons: &[usize],
        push_to_talk: Option<&[usize]>,
    ) -> String {
        let mut source = format!("app_id = '{app_id}'\n");
        if let Some(push_to_talk) = push_to_talk {
            let names: Vec<String> = push_to_talk
                .iter()
                .map(|button| format!("'{}'", BUTTONS[*button].1))
                .collect();
            writeln!(source, "push_to_talk = [{}]", names.join(", ")).unwrap();
        }
        source.push_str("[buttons]\n");
        for button in buttons {
            let name = BUTTONS[*button].1;
            writeln!(source, "{name} = {{ text = '{}' }}", marker(index, name)).unwrap();
        }
        source
    }

    /// The global context is never focused by a client, so its mappings are pressed with an
    /// unrelated client focused.
    fn press(contexts: &[Context], app_id: &str, button: ButtonName) -> Vec<Action> {
        let focused = if app_id == "*" { "unrelated-client" } else { app_id };
        let (state, _) = step(
            State::with_contexts(contexts.to_vec()),
            &Event::Focus {
                app_id: Some(focused.into()),
                title: String::new(),
            },
        );
        step(
            state,
            &Event::Button {
                name: button,
                state: ButtonState::Pressed,
            },
        )
        .1
    }

    fn contexts_strategy() -> impl Strategy<Value = Vec<(String, Vec<usize>, Option<Vec<usize>>)>> {
        prop::collection::vec(
            (
                prop::sample::select(vec!["*", "a", "b"]).prop_map(String::from),
                prop::collection::btree_set(0..BUTTONS.len(), 0..=6)
                    .prop_map(|set| set.into_iter().collect()),
                prop::option::of(
                    prop::collection::btree_set(0..BUTTONS.len(), 0..=3)
                        .prop_map(|set| set.into_iter().collect()),
                ),
            ),
            1..=4,
        )
    }

    proptest! {
        #[test]
        fn a_mapping_fires_exactly_when_it_is_not_reported_shadowed(generated in contexts_strategy()) {
            let contexts: Vec<Context> = generated
                .iter()
                .enumerate()
                .map(|(index, (app_id, buttons, push_to_talk))| {
                    parse_context(&context_source(index, app_id, buttons, push_to_talk.as_deref())).unwrap()
                })
                .collect();
            let report = shadowed(&contexts);
            // The dispatch layer hands a push-to-talk button to speech before `step` sees it.
            let global_push_to_talk: Vec<ButtonName> = generated
                .iter()
                .find(|(app_id, _, _)| app_id == "*")
                .and_then(|(_, _, list)| list.as_ref())
                .map_or_else(
                    || vec![ButtonName::Record],
                    |list| list.iter().map(|position| BUTTONS[*position].0).collect(),
                );
            for (index, (app_id, buttons, _)) in generated.iter().enumerate() {
                let context_shadowed = report
                    .iter()
                    .any(|found| matches!(found, Shadowing::Context { context, .. } if *context == index));
                for position in buttons {
                    let (name, snake) = BUTTONS[*position];
                    let reported = context_shadowed || report.iter().any(|found| {
                        matches!(found, Shadowing::Button { context, button: shadowed_button, .. } if *context == index && *shadowed_button == name)
                    });
                    let fired = !global_push_to_talk.contains(&name)
                        && press(&contexts, app_id, name)
                        == vec![Action::Text { text: Text::new(marker(index, snake)).unwrap() }];
                    prop_assert_eq!(fired, !reported, "context {} button {}", index, snake);
                }
            }
        }
    }

    #[test]
    fn reports_name_the_layer_that_wins() {
        let contexts = [
            parse_context("app_id = '*'\npush_to_talk = ['start']\n[buttons]\nsouth = { key = 'tab' }\n").unwrap(),
            parse_context("app_id = 'a'\n[buttons]\nsouth = { key = 'tab' }\nguide = { key = 'tab' }\nstart = { key = 'tab' }\neast = { key = 'tab' }\n").unwrap(),
            parse_context("app_id = 'a'\n[buttons]\neast = { key = 'tab' }\n").unwrap(),
        ];
        assert_eq!(
            shadowed(&contexts),
            vec![
                Shadowing::Button {
                    context: 0,
                    button: ButtonName::South,
                    owner: ButtonOwner::GlobalEnter
                },
                Shadowing::Button {
                    context: 1,
                    button: ButtonName::Guide,
                    owner: ButtonOwner::EscapeChord
                },
                Shadowing::Button {
                    context: 1,
                    button: ButtonName::Start,
                    owner: ButtonOwner::PushToTalk
                },
                Shadowing::Context {
                    context: 2,
                    earlier: 1
                },
            ]
        );
    }

    #[test]
    fn a_focused_contexts_south_replaces_the_global_enter() {
        let contexts = [
            parse_context("app_id = 'a'\n[buttons]\nsouth = { key = 'tab' }\n").unwrap(),
            parse_context("app_id = 'b'\n[buttons]\nsouth = { key = 'enter', mods = ['ctrl'] }\n").unwrap(),
        ];
        assert_eq!(shadowed(&contexts), vec![]);
    }

    #[test]
    fn global_south_mapped_to_plain_enter_is_not_reported() {
        let contexts = [parse_context("app_id = '*'\n[buttons]\nsouth = { key = 'enter' }\n").unwrap()];
        assert_eq!(shadowed(&contexts), vec![]);
    }

    #[test]
    fn record_is_push_to_talk_without_a_global_context() {
        let contexts = [parse_context("app_id = 'a'\n[buttons]\nrecord = { key = 'tab' }\n").unwrap()];
        assert_eq!(
            shadowed(&contexts),
            vec![Shadowing::Button {
                context: 0,
                button: ButtonName::Record,
                owner: ButtonOwner::PushToTalk
            }]
        );
    }
}
