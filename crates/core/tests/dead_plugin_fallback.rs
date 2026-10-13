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
    enum Op {
        Request(usize),
        Reply(usize, Reply),
        Tick(u64),
        Crash(usize),
        Restart(usize),
    }

    fn op() -> impl Strategy<Value = Op> {
        let reply = prop_oneof![Just(Reply::Empty), Just(Reply::Invalid), Just(Reply::Decided)];
        prop_oneof![
            4 => (0..PLUGINS).prop_map(Op::Request),
            3 => (0_usize..64, reply).prop_map(|(index, reply)| Op::Reply(index, reply)),
            3 => (0_u64..2500).prop_map(Op::Tick),
            1 => (0..PLUGINS).prop_map(Op::Crash),
            1 => (0..PLUGINS).prop_map(Op::Restart),
        ]
    }

    fn fallback(request_id: u64) -> Action {
        Action::Notice {
            text: Text::new(format!("request {request_id}")).unwrap(),
        }
    }

    fn decided() -> Vec<Action> {
        vec![Action::Key { key: KeyName::Enter }]
    }

    /// Reference model: pending requests in arrival order with their remaining budget.
    #[derive(Debug, Default)]
    struct Model {
        pending: Vec<(RequestKey, Duration)>,
        issued: Vec<RequestKey>,
        orphaned: Vec<RequestKey>,
        dead: [bool; PLUGINS],
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

    fn count(actions: &[Action], counts: &mut Vec<u64>) {
        for action in actions {
            if let Action::Notice { text } = action
                && let Some(id) = text.as_str().strip_prefix("request ")
            {
                let id: usize = id.parse().unwrap();
                counts[id] = counts[id].saturating_add(1);
            }
        }
    }

    proptest! {
        #[test]
        fn every_request_resolves_exactly_once(ops in prop::collection::vec(op(), 1..120)) {
            let mut state = State::new();
            let mut model = Model::default();
            let mut fallbacks: Vec<u64> = Vec::new();
            let mut decisions: Vec<u64> = Vec::new();
            for op in ops {
                let (next, actions, expected) = match op {
                    Op::Request(plugin) => {
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
                    Op::Reply(index, reply) => {
                        let Some(&key) = model.issued.get(index) else {
                            continue;
                        };
                        // A crashed plugin, or the dead incarnation before a restart, never replies.
                        if model.dead[key.plugin] || model.orphaned.contains(&key) {
                            continue;
                        }
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
                    Op::Tick(millis) => {
                        let elapsed = Duration::from_millis(millis);
                        let expected = model.tick(elapsed);
                        let (next, actions, rejection) = plugin::step(state, Input::Tick { elapsed });
                        prop_assert!(rejection.is_none(), "a tick rejects nothing");
                        (next, actions, expected)
                    }
                    Op::Crash(plugin) => {
                        model.dead[plugin] = true;
                        continue;
                    }
                    Op::Restart(plugin) => {
                        // A restarted plugin gets fresh request ids; the old ones stay unanswered.
                        model.dead[plugin] = false;
                        let old: Vec<RequestKey> =
                            model.issued.iter().filter(|key| key.plugin == plugin).copied().collect();
                        model.orphaned.extend(old);
                        continue;
                    }
                };
                prop_assert_eq!(&actions, &expected, "core output matches the reference model");
                count(&actions, &mut fallbacks);
                state = next;
            }
            let (_, actions, _) = plugin::step(state, Input::Tick { elapsed: SPEECH_BUDGET });
            prop_assert_eq!(&actions, &model.tick(SPEECH_BUDGET), "final timeout matches the model");
            count(&actions, &mut fallbacks);
            for (id, (fell_through, chosen)) in fallbacks.iter().zip(&decisions).enumerate() {
                prop_assert_eq!(fell_through.saturating_add(*chosen), 1, "request {} must resolve exactly once", id);
            }
        }
    }
}
