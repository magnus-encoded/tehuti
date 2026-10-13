//! State-machine law: a request to a plugin that crashes or hangs falls through to its fallback exactly once (#7).

#[cfg(test)]
mod dead_plugin_fallback {
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_core::{
        State,
        plugin::{self, Input, RequestKey, SPEECH_BUDGET},
    };
    use tehuti_protocol::{Action, Event, KeyName, Permission, Text};

    const PLUGINS: usize = 3;

    #[derive(Debug, Clone)]
    enum Reply {
        Empty,
        Invalid,
        Decided,
    }

    #[derive(Debug, Clone)]
    enum Operation {
        Request(usize),
        Reply(usize, Reply),
        Tick(u64),
        Crash(usize),
        Restart(usize),
    }

    fn operation() -> impl Strategy<Value = Operation> {
        let reply = prop_oneof![Just(Reply::Empty), Just(Reply::Invalid), Just(Reply::Decided)];
        prop_oneof![
            4 => (0..PLUGINS).prop_map(Operation::Request),
            3 => (0_usize..64, reply).prop_map(|(index, reply)| Operation::Reply(index, reply)),
            3 => (0_u64..2500).prop_map(Operation::Tick),
            1 => (0..PLUGINS).prop_map(Operation::Crash),
            1 => (0..PLUGINS).prop_map(Operation::Restart),
        ]
    }

    fn fallback(request_id: u64) -> Action {
        Action::Notice {
            text: Text::new(format!("request {request_id}")).unwrap(),
        }
    }

    /// A focused state that maps each spoken request to its own notice, so fallbacks stay identifiable.
    fn identified_state() -> State {
        use core::fmt::Write as _;
        let mut source = String::from("app_id = 'dead'\n[buttons]\nsouth = { key = 'enter' }\n[speech]\n");
        for request_id in 0..128 {
            writeln!(
                source,
                "'request {request_id}' = {{ notify = 'request {request_id}' }}"
            )
            .unwrap();
        }
        let context = tehuti_protocol::parse_context(&source).unwrap();
        let (state, _) = tehuti_core::step(
            State::with_contexts(vec![context]),
            &Event::Focus {
                app_id: Some("dead".into()),
                title: String::new(),
            },
        );
        state
    }

    fn decided() -> Vec<Action> {
        vec![Action::Key { key: KeyName::Enter }]
    }

    /// Reference model: pending requests in arrival order with their remaining budget.
    #[derive(Debug, Default)]
    struct Model {
        pending: Vec<(RequestKey, Duration)>,
        issued: Vec<RequestKey>,
        next_id: u64,
    }

    impl Model {
        fn tick(&mut self, elapsed: Duration) -> Vec<Action> {
            let mut actions = Vec::new();
            self.pending.retain_mut(|(key, remaining)| {
                *remaining = remaining.saturating_sub(elapsed);
                if remaining.is_zero() {
                    actions.push(fallback(key.request_id));
                    false
                } else {
                    true
                }
            });
            actions
        }

        fn reply(&mut self, key: RequestKey, reply: &Reply) -> Vec<Action> {
            let Some(index) = self.pending.iter().position(|(pending, _)| *pending == key) else {
                return Vec::new();
            };
            let _settled = self.pending.remove(index);
            match reply {
                Reply::Decided => decided(),
                Reply::Empty | Reply::Invalid => vec![fallback(key.request_id)],
            }
        }
    }

    fn count(actions: &[Action], counts: &mut [u64]) {
        for action in actions {
            if let Action::Notice { text } = action
                && let Some(index) = text.as_str().strip_prefix("request ")
            {
                let index: usize = index.parse().unwrap();
                counts[index] = counts[index].saturating_add(1);
            }
        }
    }

    proptest! {
        #[test]
        fn exit_releases_fallbacks_in_arrival_order_before_later_actions(
            owners in prop::collection::vec(0..PLUGINS, 1..80),
            dead in 0..PLUGINS,
        ) {
            let mut state = identified_state();
            let mut expected = Vec::new();
            let mut keys = Vec::new();
            for (index, owner) in owners.into_iter().enumerate() {
                let key = RequestKey { plugin: owner, request_id: u64::try_from(index).unwrap() };
                keys.push(key);
                if owner == dead {
                    expected.push(fallback(key.request_id));
                }
                let (next, actions, _) = plugin::step(state, Input::Request {
                    key,
                    event: Event::Speech { text: Text::new(format!("request {}", key.request_id)).unwrap() },
                    permissions: vec![Permission::Key],
                });
                prop_assert!(actions.is_empty());
                state = next;
            }
            let (next, actions, _) = plugin::step(state, Input::Exited { plugin: dead });
            prop_assert_eq!(actions, expected);
            state = next;
            let (after_duplicate, duplicate_actions, _) = plugin::step(state, Input::Exited { plugin: dead });
            prop_assert!(duplicate_actions.is_empty());
            state = after_duplicate;
            for key in keys.into_iter().filter(|key| key.plugin == dead) {
                let (after_reply, reply_actions, _) = plugin::step(state, Input::Reply { key, result: Ok(decided()) });
                prop_assert!(reply_actions.is_empty(), "a dead incarnation releases nothing");
                state = after_reply;
            }
            let (_, later_actions) = tehuti_core::step(state, &Event::Button {
                name: tehuti_protocol::ButtonName::South,
                state: tehuti_protocol::ButtonState::Pressed,
            });
            prop_assert_eq!(later_actions, decided());
        }

        #[test]
        fn every_request_resolves_exactly_once(ops in prop::collection::vec(operation(), 1..120)) {
            let mut state = identified_state();
            let mut model = Model::default();
            let mut fallbacks: Vec<u64> = Vec::new();
            let mut decisions: Vec<u64> = Vec::new();
            for operation in ops {
                let (next, actions, expected) = match operation {
                    Operation::Request(plugin) => {
                        let key = RequestKey { plugin, request_id: model.next_id };
                        model.next_id = model.next_id.saturating_add(1);
                        model.issued.push(key);
                        fallbacks.push(0);
                        decisions.push(0);
                        model.pending.push((key, SPEECH_BUDGET));
                        let (next, actions, rejection) = plugin::step(state, Input::Request {
                            key,
                            event: Event::Speech {
                                text: Text::new(format!("request {}", key.request_id)).unwrap(),
                            },
                            permissions: vec![Permission::Key],
                        });
                        prop_assert!(rejection.is_none(), "a valid request is accepted");
                        (next, actions, Vec::new())
                    }
                    Operation::Reply(index, reply) => {
                        let Some(&key) = model.issued.get(index) else {
                            continue;
                        };
                        let expected = model.reply(key, &reply);
                        if expected == decided() {
                            let slot = usize::try_from(key.request_id).unwrap();
                            decisions[slot] = decisions[slot].saturating_add(1);
                        }
                        let result = match reply {
                            Reply::Empty => Ok(Vec::new()),
                            Reply::Invalid => Ok(vec![Action::Quit]),
                            Reply::Decided => Ok(decided()),
                        };
                        let (next, actions, _) = plugin::step(state, Input::Reply { key, result });
                        (next, actions, expected)
                    }
                    Operation::Tick(millis) => {
                        let elapsed = Duration::from_millis(millis);
                        let expected = model.tick(elapsed);
                        let (next, actions, rejection) = plugin::step(state, Input::Tick { elapsed });
                        prop_assert!(rejection.is_none(), "a tick rejects nothing");
                        (next, actions, expected)
                    }
                    Operation::Crash(plugin) | Operation::Restart(plugin) => {
                        let mut expected = Vec::new();
                        model.pending.retain(|(key, _)| {
                            if key.plugin == plugin {
                                expected.push(fallback(key.request_id));
                                false
                            } else {
                                true
                            }
                        });
                        let (next, actions, _) = plugin::step(state, Input::Exited { plugin });
                        (next, actions, expected)
                    }

                };
                prop_assert_eq!(&actions, &expected, "core output matches the reference model");
                count(&actions, &mut fallbacks);
                state = next;
            }
            let (_, actions, _) = plugin::step(state, Input::Tick { elapsed: SPEECH_BUDGET });
            prop_assert_eq!(&actions, &model.tick(SPEECH_BUDGET), "final timeout matches the model");
            count(&actions, &mut fallbacks);
            for (index, (fell_through, chosen)) in fallbacks.iter().zip(&decisions).enumerate() {
                prop_assert_eq!(fell_through.saturating_add(*chosen), 1, "request {} must resolve exactly once", index);
            }
        }
    }
}
