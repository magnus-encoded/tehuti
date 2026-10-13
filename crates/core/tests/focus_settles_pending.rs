//! Laws for a focus change while plugin decisions are pending (issue #39).

#[cfg(test)]
mod focus_settles_pending {
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_core::{
        State,
        plugin::{self, Input, RequestKey},
        step,
    };
    use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName, Permission, Text};

    fn focus(app_id: &str) -> Event {
        Event::Focus {
            app_id: Some(app_id.into()),
            title: String::new(),
        }
    }

    fn east() -> Event {
        Event::Button {
            name: ButtonName::East,
            state: ButtonState::Pressed,
        }
    }

    fn editor(east_text: &Text) -> State {
        let source = format!(
            "app_id = 'editor'\n[buttons.east]\ntext = {}\n",
            toml::Value::String(east_text.as_str().into())
        );
        let context = tehuti_protocol::parse_context(&source).unwrap();
        step(State::with_contexts(vec![context]), &focus("editor")).0
    }

    /// Queues one request per entry; `Some(text)` is speech, `None` an East press.
    /// Returns the state, the keys and the fallbacks in arrival order.
    fn pending(east_text: &Text, series: &[Option<Text>]) -> (State, Vec<RequestKey>, Vec<Action>) {
        let mut state = editor(east_text);
        let mut keys = Vec::new();
        let mut fallbacks = Vec::new();
        for (index, entry) in series.iter().enumerate() {
            let key = RequestKey {
                plugin: index % 3,
                request_id: u64::try_from(index).unwrap(),
            };
            let (event, fallback) = entry.as_ref().map_or_else(
                || {
                    (
                        east(),
                        Action::Text {
                            text: east_text.clone(),
                        },
                    )
                },
                |text| {
                    (
                        Event::Speech { text: text.clone() },
                        Action::Notice { text: text.clone() },
                    )
                },
            );
            let (next, actions, rejection) = plugin::step(
                state,
                Input::Request {
                    key,
                    event,
                    permissions: vec![Permission::Key],
                },
            );
            assert!(actions.is_empty(), "requests defer their fallback");
            assert!(rejection.is_none(), "valid requests are accepted");
            state = next;
            keys.push(key);
            fallbacks.push(fallback);
        }
        (state, keys, fallbacks)
    }

    fn series() -> impl Strategy<Value = Vec<Option<Text>>> {
        prop::collection::vec(prop::option::of(any::<Text>()), 1..20)
    }

    #[test]
    fn focus_releases_a_pending_press_instead_of_clearing_it() {
        let east_text = Text::new("east".into()).unwrap();
        let (state, _, fallbacks) = pending(&east_text, &[None]);
        let (_, released) = step(state, &focus("other"));
        assert_eq!(
            released,
            vec![Action::Text { text: east_text }],
            "a focus change must settle the pending press, not drop it"
        );
        assert_eq!(released, fallbacks);
    }

    proptest! {
        #[test]
        fn focus_equals_timeout_then_focus(
            east_text in any::<Text>(),
            series in series(),
            waited in 0_u64..100,
            target in "[a-z]{1,12}",
            future in prop::collection::vec(any::<Event>(), 0..20),
        ) {
            let (state, _, fallbacks) = pending(&east_text, &series);
            let (state, early, _) = plugin::step(state, Input::Tick { elapsed: Duration::from_millis(waited) });
            prop_assert!(early.is_empty());

            let (mut focused, focus_actions) = step(state.clone(), &focus(&target));
            let (timed, mut timeout_actions, _) = plugin::step(state, Input::Tick { elapsed: Duration::from_secs(2) });
            let (mut after, then_focus) = step(timed, &focus(&target));
            timeout_actions.extend(then_focus);

            prop_assert_eq!(&focus_actions, &timeout_actions);
            prop_assert_eq!(&focus_actions, &fallbacks, "each fallback once, in arrival order");
            for event in future {
                let (next_focused, focused_actions) = step(focused, &event);
                let (next_after, after_actions) = step(after, &event);
                prop_assert_eq!(focused_actions, after_actions);
                focused = next_focused;
                after = next_after;
            }
        }

        #[test]
        fn late_reply_after_focus_releases_nothing(
            east_text in any::<Text>(),
            series in series(),
            elapsed in 0_u64..2000,
        ) {
            let (state, keys, _) = pending(&east_text, &series);
            let (mut state, _) = step(state, &focus("other"));
            let (after_tick, timed, _) = plugin::step(state, Input::Tick { elapsed: Duration::from_millis(elapsed) });
            prop_assert!(timed.is_empty(), "settled decisions cannot time out again");
            state = after_tick;
            for key in keys {
                for result in [Ok(Vec::new()), Ok(vec![Action::Key { key: KeyName::Enter }])] {
                    let (after_reply, late, _) = plugin::step(state, Input::Reply { key, result });
                    prop_assert!(late.is_empty(), "a late reply after focus releases nothing");
                    state = after_reply;
                }
            }
        }
    }
}
