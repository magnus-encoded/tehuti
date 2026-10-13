//! Independent laws for plugin deadlines and reply identity.

#[cfg(test)]
mod plugin_latency {
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_core::{
        State,
        plugin::{self, Input, RequestKey},
        step,
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Permission, Text};

    fn unmapped_notice() -> Action {
        Action::Notice {
            text: Text::new("NO SPEECH MAPPING".into()).unwrap(),
        }
    }

    fn speech(text: Text) -> Event {
        Event::Speech { text }
    }

    fn request(state: State, key: RequestKey, event: Event) -> State {
        let (state, actions, rejection) = plugin::step(
            state,
            Input::Request {
                key,
                event,
                permissions: vec![Permission::Key],
            },
        );
        assert!(actions.is_empty(), "requests defer their fallback");
        assert!(rejection.is_none(), "valid requests are accepted");
        state
    }

    fn saturated_pending_state(event: &Event) -> State {
        let mut state = State::new();
        for request_id in 0..256 {
            state = request(
                state,
                RequestKey {
                    plugin: 0,
                    request_id,
                },
                event.clone(),
            );
        }
        state
    }

    #[test]
    fn excess_pending_requests_fall_back_once() {
        let text = Text::new("fallback".into()).unwrap();
        let event = speech(text);
        let state = saturated_pending_state(&event);
        let excess = RequestKey {
            plugin: 0,
            request_id: 256,
        };
        let (state, fallback, _) = plugin::step(
            state,
            Input::Request {
                key: excess,
                event,
                permissions: vec![Permission::Key],
            },
        );
        assert_eq!(fallback, vec![unmapped_notice()]);
        let (_, duplicate, _) = plugin::step(
            state,
            Input::Reply {
                key: excess,
                result: Ok(vec![Action::Key { key: KeyName::Enter }]),
            },
        );
        assert_eq!(duplicate, Vec::<Action>::new());
    }

    #[test]
    fn resolving_and_expiring_requests_free_pending_capacity() {
        let text = Text::new("fallback".into()).unwrap();
        let fallback = vec![unmapped_notice()];
        let event = speech(text);
        let state = saturated_pending_state(&event);
        let (state, declined, _) = plugin::step(
            state,
            Input::Reply {
                key: RequestKey {
                    plugin: 0,
                    request_id: 0,
                },
                result: Ok(Vec::new()),
            },
        );
        assert_eq!(declined, fallback);
        let state = request(
            state,
            RequestKey {
                plugin: 0,
                request_id: 257,
            },
            event.clone(),
        );
        let (state, expired, _) = plugin::step(
            state,
            Input::Tick {
                elapsed: Duration::from_secs(2),
            },
        );
        assert_eq!(expired, vec![fallback[0].clone(); 256]);
        let state = request(
            state,
            RequestKey {
                plugin: 0,
                request_id: 258,
            },
            event,
        );
        let (_, resolved, _) = plugin::step(
            state,
            Input::Reply {
                key: RequestKey {
                    plugin: 0,
                    request_id: 258,
                },
                result: Ok(Vec::new()),
            },
        );
        assert_eq!(resolved, fallback);
    }

    proptest! {
        #[test]
        fn button_fallback_fires_at_exactly_100_milliseconds(
            text in any::<Text>(),
            before in 0_u64..100,
            request_id in 0_u64..1000,
        ) {
            let source = format!("app_id = 'editor'\n[buttons.east]\ntext = {}\n", toml::Value::String(text.as_str().into()));
            let context = tehuti_protocol::parse_context(&source).unwrap();
            let (focused, _) = step(State::with_contexts(vec![context]), &Event::Focus {
                app_id: Some("editor".into()), title: String::new(),
            });
            let key = RequestKey { plugin: 0, request_id };
            let pending = request(focused, key, Event::Button { name: ButtonName::East, state: ButtonState::Pressed });
            let (state, early, _) = plugin::step(pending, Input::Tick { elapsed: Duration::from_millis(before) });
            prop_assert!(early.is_empty(), "fallback must not fire before its deadline");
            let (state, due, _) = plugin::step(state, Input::Tick { elapsed: Duration::from_millis(100_u64.saturating_sub(before)) });
            prop_assert_eq!(due, vec![Action::Text { text }]);
            let (_, late, _) = plugin::step(state, Input::Reply { key, result: Ok(vec![Action::Key { key: KeyName::Enter }]) });
            prop_assert!(late.is_empty(), "the deadline closes the request before a reply");
        }

        #[test]
        fn leaving_and_returning_to_the_same_app_invalidates_old_decisions(
            text in any::<Text>(),
            elapsed in 0_u64..2000,
        ) {
            let key = RequestKey { plugin: 0, request_id: 1 };
            let (focused, _) = step(State::new(), &Event::Focus { app_id: Some("editor".into()), title: String::new() });
            let pending = request(focused, key, speech(text));
            let (state, _) = step(pending, &Event::Focus { app_id: Some("other".into()), title: String::new() });
            let (state, _) = step(state, &Event::Focus { app_id: Some("editor".into()), title: String::new() });
            let (state, timed, _) = plugin::step(state, Input::Tick { elapsed: Duration::from_millis(elapsed) });
            prop_assert!(timed.is_empty(), "old fallback cannot cross a focus generation");
            let (_, replied, _) = plugin::step(state, Input::Reply { key, result: Ok(vec![Action::Key { key: KeyName::Enter }]) });
            prop_assert!(replied.is_empty(), "returning to the same app must not revive an old reply");
        }

        #[test]
        fn expiry_at_the_deadline_equals_decline_for_actions_and_future_state(
            text in any::<Text>(),
            contexts in prop::collection::vec("[a-z]{1,12}", 0..12),
            future in prop::collection::vec(any::<Event>(), 0..30),
        ) {
            let contexts = contexts.into_iter().map(|app_id| {
                tehuti_protocol::parse_context(&format!("app_id = '{app_id}'\n[buttons]\n")).unwrap()
            }).collect();
            let key = RequestKey { plugin: 0, request_id: 1 };
            let pending = request(State::with_contexts(contexts), key, speech(text));
            let (mut declined, decline_actions, rejection) = plugin::step(pending.clone(), Input::Reply {
                key,
                result: Ok(Vec::new()),
            });
            let (mut expired, expiry_actions, expiry_rejection) = plugin::step(pending, Input::Tick {
                elapsed: Duration::from_secs(2),
            });
            prop_assert!(rejection.is_none());
            prop_assert!(expiry_rejection.is_none());
            prop_assert_eq!(&decline_actions, &vec![unmapped_notice()]);
            prop_assert_eq!(expiry_actions, decline_actions);
            for event in future {
                let (next_declined, declined_actions) = step(declined, &event);
                let (next_expired, expired_actions) = step(expired, &event);
                prop_assert_eq!(expired_actions, declined_actions);
                declined = next_declined;
                expired = next_expired;
            }
        }

        #[test]
        fn permission_failure_rejects_the_whole_batch_and_releases_fallback_once(
            text in any::<Text>(),
            prefix in prop::collection::vec(any::<KeyName>(), 0..20),
        ) {
            let key = RequestKey { plugin: 0, request_id: 1 };
            let pending = request(State::new(), key, speech(text.clone()));
            let mut actions: Vec<_> = prefix.into_iter().map(|name| Action::Key { key: name }).collect();
            actions.push(Action::Text { text });
            let (resolved, delivered, rejection) = plugin::step(pending, Input::Reply { key, result: Ok(actions) });
            prop_assert_eq!(delivered, vec![unmapped_notice()]);
            prop_assert!(rejection.as_ref().is_some_and(|rejected| rejected.key == key && rejected.reason.contains("text")), "a denied action names its request and reason");
            let (_, repeated, _) = plugin::step(resolved, Input::Reply {
                key,
                result: Ok(vec![Action::Key { key: KeyName::Enter }]),
            });
            prop_assert!(repeated.is_empty(), "a resolved request cannot deliver twice");
        }

        #[test]
        fn expired_reply_cannot_consume_a_later_request(
            first in any::<Text>(),
            second in any::<Text>(),
            plugin_index in 0_usize..8,
            first_id in 1_u64..1000,
        ) {
            let old = RequestKey { plugin: plugin_index, request_id: first_id };
            let new = RequestKey { plugin: plugin_index, request_id: first_id.saturating_add(1) };
            let state = request(State::new(), old, speech(first));
            let (state, _, _) = plugin::step(state, Input::Tick { elapsed: Duration::from_secs(2) });
            let state = request(state, new, speech(second));
            let (state, late, _) = plugin::step(state, Input::Reply {
                key: old,
                result: Ok(vec![Action::Key { key: KeyName::Enter }]),
            });
            prop_assert!(late.is_empty(), "late replies cannot deliver or release another event");
            let (_, delivered, _) = plugin::step(state, Input::Reply { key: new, result: Ok(Vec::new()) });
            prop_assert_eq!(delivered, vec![unmapped_notice()]);
        }
    }
}
