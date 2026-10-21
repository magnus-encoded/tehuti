//! Properties of the core step over generated event series.

#[cfg(test)]
mod step {
    use core::fmt::Write;

    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName};

    fn run(events: &[Event]) -> Vec<Action> {
        run_from(State::new(), events)
    }

    fn run_from(mut state: State, events: &[Event]) -> Vec<Action> {
        let mut actions = Vec::new();
        for event in events {
            let (next, emitted) = step(state, event);
            state = next;
            actions.extend(emitted);
        }
        actions
    }

    const A_PRESSED: Event = Event::Button {
        name: ButtonName::South,
        state: ButtonState::Pressed,
    };

    proptest! {
        #[test]
        fn step_returns_for_any_event_series(
            events in prop::collection::vec(any::<Event>(), 0..200),
            configurations in prop::collection::vec(prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=15), 0..20),
        ) {
            let contexts = configurations.iter().map(|mappings| generated_context("test", mappings)).collect();
            let _actions = run_from(State::with_contexts(contexts), &events);
        }

        #[test]
        fn outside_the_menu_each_a_press_sends_one_enter_and_nothing_else_reaches_the_client(
            events in prop::collection::vec(any::<Event>().prop_filter("outside the system menu", |event| !matches!(event, Event::Button { name: ButtonName::Guide, .. })), 0..200),
        ) {
            let a_presses = events.iter().filter(|event| **event == A_PRESSED).count();
            let keys: Vec<_> = run(&events).into_iter().filter(|action| matches!(action, Action::Key { .. })).collect();
            prop_assert_eq!(keys, vec![Action::Key { key: KeyName::Enter, mods: Vec::new() }; a_presses]);
        }
    }

    proptest! {
        #[test]
        fn context_text_preserves_unicode_when_its_mapping_fires(
            text in any::<tehuti_protocol::Text>(),
            app_id in "[a-z]{1,30}",
            name in any::<ButtonName>().prop_filter("context-routed button", |name| !matches!(name, ButtonName::South | ButtonName::Guide | ButtonName::Record)),
            mappings in prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=15),
        ) {
            let mut source = format!("app_id = {}\n[buttons]\n", toml::Value::String(app_id.clone()));
            for (button, key) in mappings {
                if button != name {
                    writeln!(source, "[buttons.{}]\nkey = '{key:?}'", button_name(button)).unwrap();
                }
            }
            writeln!(source, "[buttons.{}]\ntext = {}", button_name(name), toml::Value::String(text.as_str().into())).unwrap();
            let context = tehuti_protocol::parse_context(&source).unwrap();
            let (state, _) = step(State::with_contexts(vec![context]), &Event::Focus { app_id: Some(app_id), title: String::new() });
            let (_, actions) = step(state, &Event::Button { name, state: ButtonState::Pressed });
            prop_assert_eq!(actions, vec![Action::Text { text }]);
        }
    }

    /// A generated plugin: the app it matches, whether it subscribes to buttons
    /// and whether its handshake completed. The compositor reports whether any
    /// such plugin is live for the focused app; consume or decline is the
    /// plugin's reply and never feeds back into the core step.
    #[derive(Debug, Clone)]
    struct Plugin {
        app_id: &'static str,
        subscribes: bool,
        ready: bool,
    }

    const APP_IDS: [&str; 3] = ["a", "b", "c"];

    fn plugin() -> impl Strategy<Value = Plugin> {
        (
            prop::sample::select(APP_IDS.to_vec()),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(app_id, subscribes, ready)| Plugin {
                app_id,
                subscribes,
                ready,
            })
    }

    fn routed_event() -> impl Strategy<Value = Event> {
        prop_oneof![
            any::<Event>(),
            prop::option::of(prop::sample::select(APP_IDS.to_vec())).prop_map(|app_id| Event::Focus {
                app_id: app_id.map(str::to_owned),
                title: String::new()
            }),
        ]
    }

    proptest! {
        #[test]
        fn plugin_routing_obeys_the_layer_laws_for_every_configuration(
            configurations in prop::collection::vec((prop::sample::select(APP_IDS.to_vec()), prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=15)), 0..6),
            plugins in prop::collection::vec(plugin(), 0..6),
            events in prop::collection::vec(routed_event(), 0..100),
            millis in 3000u64..100_000,
        ) {
            let contexts: Vec<_> = configurations.iter().map(|(app_id, mappings)| generated_context(app_id, mappings)).collect();
            let mut routed = State::with_contexts(contexts.clone());
            let mut plain = State::with_contexts(contexts);
            let mut bare = State::new();
            let mut focus: Option<String> = None;
            let mut routed_quits = Vec::new();
            let mut plain_quits = Vec::new();
            let series = events.iter().cloned().chain([guide(ButtonState::Pressed), start(), tick(millis)]);
            for (index, event) in series.enumerate() {
                if let Event::Focus { app_id, .. } = &event { focus.clone_from(app_id); }
                let active = plugins.iter().any(|plugin| plugin.ready && plugin.subscribes && focus.as_deref() == Some(plugin.app_id));
                let (next_routed, routed_actions, sent) = tehuti_core::step_with_plugin(routed, &event, active);
                let (next_plain, plain_actions) = step(plain, &event);
                let (next_bare, bare_actions) = step(bare, &event);
                let focus_event = Event::Focus { app_id: focus.clone(), title: String::new() };
                let routed_control = step(next_routed.clone(), &focus_event).0;
                let plain_control = step(next_plain.clone(), &focus_event).0;
                prop_assert_eq!(routed_control, plain_control, "routing preserves focus, contexts and controller holds after voice feedback clears");
                if sent {
                    prop_assert!(routed_actions.is_empty(), "an event sent to a plugin never also reaches the context layer");
                    prop_assert!(!bare_actions.iter().any(|action| matches!(action, Action::Key { .. } | Action::Quit)), "only events no layer above the context consumed reach a plugin");
                    prop_assert!(active, "nothing is sent while no plugin is live");
                } else {
                    prop_assert_eq!(&routed_actions, &plain_actions, "a plugin set that is not sent the event leaves output untouched");
                }
                if bare_actions.iter().any(|action| matches!(action, Action::Key { .. } | Action::Quit)) || plain_actions.contains(&Action::Quit) {
                    prop_assert!(!sent, "whenever an escape or global action fires, nothing is sent to a plugin");
                }
                if matches!(event, Event::Tick { .. }) {
                    prop_assert!(!sent, "Tick never crosses the wire");
                }
                routed_quits.extend(routed_actions.iter().filter(|action| **action == Action::Quit).map(|_| index));
                plain_quits.extend(plain_actions.iter().filter(|action| **action == Action::Quit).map(|_| index));
                routed = next_routed;
                plain = next_plain;
                bare = next_bare;
            }
            prop_assert_eq!(&routed_quits, &plain_quits, "no plugin set moves or suppresses escape");
            prop_assert_eq!(routed_quits.last().copied(), events.len().checked_add(2), "holding Guide+Start for three seconds quits under every plugin set");
        }
    }

    fn button_name(button: ButtonName) -> String {
        snake_case(&format!("{button:?}"))
    }

    fn snake_case(debug: &str) -> String {
        let mut name = String::new();
        for (index, letter) in debug.chars().enumerate() {
            if index > 0 && letter.is_uppercase() {
                name.push('_');
            }
            name.extend(letter.to_lowercase());
        }
        name
    }

    fn generated_context(
        app_id: &str,
        mappings: &std::collections::HashMap<ButtonName, KeyName>,
    ) -> tehuti_protocol::Context {
        let mut source = format!("app_id = '{app_id}'\n[buttons]\n");
        for (button, key) in mappings {
            write!(source, "[buttons.{}]\nkey = '{key:?}'\n", button_name(*button)).unwrap();
        }
        tehuti_protocol::parse_context(&source).unwrap()
    }

    proptest! {
        #[test]
        fn routing_stops_at_the_first_consuming_layer(
            mappings in prop::collection::hash_map(prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::North, ButtonName::West]), any::<KeyName>(), 0..=4),
            other_mappings in prop::collection::hash_map(prop::sample::select(vec![ButtonName::South, ButtonName::East, ButtonName::North, ButtonName::West]), any::<KeyName>(), 0..=4),
            focused in prop::option::of(prop::sample::select(vec!["test", "other", "unmatched"])),
            name in any::<ButtonName>(),
            button_state in any::<ButtonState>(),
        ) {
            let state = State::with_contexts(vec![generated_context("test", &mappings), generated_context("other", &other_mappings)]);
            let (state, focus_actions) = step(state, &Event::Focus { app_id: focused.map(str::to_owned), title: String::new() });
            prop_assert!(focus_actions.is_empty());
            let (_, actions) = step(state, &Event::Button { name, state: button_state });
            if button_state == ButtonState::Pressed && name == ButtonName::South {
                prop_assert_eq!(actions, vec![Action::Key { key: KeyName::Enter, mods: Vec::new() }], "global consumption excludes context and fallback output");
            } else if button_state == ButtonState::Pressed {
                let mapping = match focused {
                    Some("test") => mappings.get(&name),
                    Some("other") => other_mappings.get(&name),
                    _ => None,
                };
                if let Some(key) = mapping {
                    prop_assert_eq!(actions, vec![Action::Key { key: *key, mods: Vec::new() }], "context consumption excludes fallback output");
                } else {
                    prop_assert!(actions.is_empty(), "unmapped presses use the empty fallback");
                }
            } else {
                prop_assert!(actions.is_empty(), "releases use the empty fallback");
            }
        }
    }

    fn guide(button_state: ButtonState) -> Event {
        Event::Button {
            name: ButtonName::Guide,
            state: button_state,
        }
    }

    #[test]
    fn both_buttons_must_stay_held_for_three_seconds() {
        for released in [ButtonName::Guide, ButtonName::Start] {
            let mut state = State::new();
            for event in [
                guide(ButtonState::Pressed),
                tick(4000),
                start(),
                tick(2999),
                Event::Button {
                    name: released,
                    state: ButtonState::Released,
                },
                tick(4000),
            ] {
                let (next, actions) = step(state, &event);
                state = next;
                assert!(!actions.contains(&Action::Quit));
            }
        }
    }

    #[test]
    fn start_alone_reaches_the_app_and_only_joint_hold_counts() {
        let context =
            tehuti_protocol::parse_context("app_id = 'test'\n[buttons.start]\nkey = 'space'").unwrap();
        let (state, _) = step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            },
        );
        let (state, start_actions) = step(state, &start());
        assert_eq!(
            start_actions,
            vec![Action::Key {
                key: KeyName::Space,
                mods: Vec::new()
            }]
        );
        let (state, solo_actions) = step(state, &tick(4000));
        assert_eq!(solo_actions, Vec::<Action>::new());
        let (state, guide_actions) = step(state, &guide(ButtonState::Pressed));
        assert_eq!(guide_actions, Vec::<Action>::new());
        let (state, early_actions) = step(state, &tick(2999));
        assert_eq!(early_actions, Vec::<Action>::new());
        let (_, quit_actions) = step(state, &tick(1));
        assert_eq!(quit_actions, vec![Action::Quit]);
    }

    #[test]
    fn controller_disconnect_clears_both_holds_and_elapsed_time() {
        for first in [ButtonName::Guide, ButtonName::Start] {
            let mut state = State::new();
            for event in [guide(ButtonState::Pressed), start(), tick(2999)] {
                state = step(state, &event).0;
            }
            state.controller_disconnected();
            let (state, _) = step(
                state,
                &Event::Button {
                    name: first,
                    state: ButtonState::Pressed,
                },
            );
            let (state, reconnect_actions) = step(state, &tick(4000));
            assert_eq!(reconnect_actions, Vec::<Action>::new());
            let second = if first == ButtonName::Guide {
                start()
            } else {
                guide(ButtonState::Pressed)
            };
            let (state, _) = step(state, &second);
            let (state, fresh_early_actions) = step(state, &tick(2999));
            assert_eq!(fresh_early_actions, Vec::<Action>::new());
            let (_, fresh_quit_actions) = step(state, &tick(1));
            assert_eq!(fresh_quit_actions, vec![Action::Quit]);
        }
    }

    fn start() -> Event {
        Event::Button {
            name: ButtonName::Start,
            state: ButtonState::Pressed,
        }
    }

    fn tick(millis: u64) -> Event {
        Event::Tick {
            elapsed: core::time::Duration::from_millis(millis),
        }
    }

    proptest! {
        #[test]
        fn every_held_guide_series_quits_once_at_three_seconds(
            configurations in prop::collection::vec(
                prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=15), 1..20),
            guide_key in any::<KeyName>(),
            series in prop::collection::vec(
                (
                    prop::collection::vec(prop_oneof![
                        (any::<ButtonName>().prop_filter("Guide stays held", |name| !matches!(name, ButtonName::Guide | ButtonName::Start)), any::<ButtonState>())
                            .prop_map(|(name, state)| Event::Button { name, state }),
                        (prop::option::of(prop::sample::select(vec!["test", "other", "unmatched"])), any::<String>())
                            .prop_map(|(app_id, title)| Event::Focus { app_id: app_id.map(str::to_owned), title }),
                    ], 0..5),
                    50u64..=100,
                ),
                60..150,
            ),
        ) {
            let contexts = configurations.into_iter().enumerate().map(|(index, mut mappings)| {
                let _previous = mappings.insert(ButtonName::Guide, guide_key);
                generated_context(if index % 2 == 0 { "test" } else { "other" }, &mappings)
            }).collect();
            let (state, _) = step(State::with_contexts(contexts), &Event::Focus {
                app_id: Some("test".into()), title: String::new(),
            });
            let (state, press_actions) = step(state, &guide(ButtonState::Pressed));
            prop_assert!(press_actions.is_empty(), "Guide never fires its context mapping");
            let (mut state, _) = step(state, &start());
            let mut elapsed = core::time::Duration::ZERO;
            let mut quits = 0usize;
            for (other_events, millis) in series {
                for event in other_events.into_iter().chain(core::iter::once(tick(millis))) {
                    if let Event::Tick { elapsed: delta } = &event {
                        elapsed = elapsed.checked_add(*delta).unwrap();
                    }
                    let (next, actions) = step(state, &event);
                    state = next;
                    quits = quits.checked_add(actions.iter().filter(|action| **action == Action::Quit).count()).unwrap();
                    prop_assert_eq!(quits, usize::from(elapsed >= core::time::Duration::from_secs(3)),
                        "every prefix emits no Quit before three seconds and exactly one thereafter");
                }
            }
            prop_assert!(elapsed >= core::time::Duration::from_secs(3));
            prop_assert_eq!(quits, 1);
        }

        #[test]
        fn releasing_early_cancels_the_hold(
            elapsed in 0u64..3000,
            mappings in prop::collection::hash_map(any::<ButtonName>(), any::<KeyName>(), 0..=15),
        ) {
            let mut state = State::with_contexts(vec![generated_context("test", &mappings)]);
            for event in [tick(u64::MAX), guide(ButtonState::Pressed), start(), tick(elapsed), guide(ButtonState::Released), tick(u64::MAX), guide(ButtonState::Pressed), tick(2999)] {
                let (next, actions) = step(state, &event);
                state = next;
                prop_assert!(actions.is_empty());
            }
            let (_, actions) = step(state, &tick(1));
            prop_assert_eq!(actions, vec![Action::Quit]);
        }
    }

    #[test]
    fn first_context_with_matching_app_id_wins() {
        let first = tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'tab'").unwrap();
        let second = tehuti_protocol::parse_context(
            "app_id = 'test'\n[buttons.east]\nkey = 'escape'\n[buttons.north]\nkey = 'space'",
        )
        .unwrap();
        let (state, _) = step(
            State::with_contexts(vec![first, second]),
            &Event::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            },
        );
        let (state, actions) = step(
            state,
            &Event::Button {
                name: ButtonName::East,
                state: ButtonState::Pressed,
            },
        );
        assert_eq!(
            actions,
            vec![Action::Key {
                key: KeyName::Tab,
                mods: Vec::new()
            }]
        );
        let (_, unmapped_actions) = step(
            state,
            &Event::Button {
                name: ButtonName::North,
                state: ButtonState::Pressed,
            },
        );
        assert!(
            unmapped_actions.is_empty(),
            "later duplicate contexts cannot fill missing mappings"
        );
    }

    #[test]
    fn focus_selects_and_clears_the_active_context() {
        let context = tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'tab'").unwrap();
        let mut state = State::with_contexts(vec![context]);
        for app_id in [Some("test"), Some("other"), Some("test"), None] {
            (state, _) = step(
                state,
                &Event::Focus {
                    app_id: app_id.map(str::to_owned),
                    title: String::new(),
                },
            );
            let (next, actions) = step(
                state,
                &Event::Button {
                    name: ButtonName::East,
                    state: ButtonState::Pressed,
                },
            );
            state = next;
            assert_eq!(
                actions,
                if app_id == Some("test") {
                    vec![Action::Key {
                        key: KeyName::Tab,
                        mods: Vec::new(),
                    }]
                } else {
                    vec![]
                },
                "only matching focus selects the context"
            );
        }
    }

    #[test]
    fn pressing_a_sends_enter() {
        assert_eq!(
            run(&[A_PRESSED]),
            vec![Action::Key {
                key: KeyName::Enter,
                mods: Vec::new()
            }],
            "the global layer maps A to Enter"
        );
    }

    #[test]
    fn releasing_a_sends_nothing() {
        let released = Event::Button {
            name: ButtonName::South,
            state: ButtonState::Released,
        };
        assert_eq!(run(&[released]), vec![], "Enter is a full tap on press");
    }
}
