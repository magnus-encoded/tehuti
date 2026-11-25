//! A plugin's exit clears everything it claimed in one reducer (#888): its pending requests fall
//! back and its invoke words go, while other plugins keep theirs.

#[cfg(test)]
mod plugin_exit_claims {
    use proptest::prelude::*;
    use tehuti_core::{
        State,
        plugin::{self, Input},
    };

    const PLUGINS: usize = 4;

    fn claims() -> impl Strategy<Value = Vec<(usize, Vec<String>)>> {
        prop::collection::vec(
            (0..PLUGINS, prop::collection::vec("[a-z]{5,9}", 0..3)),
            0..PLUGINS,
        )
        .prop_map(|mut claims| {
            claims.sort_by_key(|(plugin, _)| *plugin);
            claims.dedup_by_key(|(plugin, _)| *plugin);
            claims
        })
    }

    #[test]
    fn exit_settles_only_its_requests_and_words_together() {
        use tehuti_core::plugin::RequestKey;
        use tehuti_protocol::{Action, ButtonName, ButtonState, Event, Text, parse_context};

        let context = parse_context("app_id = '*'\n[buttons]\neast = { text = 'fallback' }").unwrap();
        let mut state = State::with_contexts(vec![context]);
        for plugin in 0..2 {
            state.set_plugin_invoke_words(plugin, vec![format!("word-{plugin}")]);
            (state, _, _) = plugin::step(
                state,
                Input::Request {
                    key: RequestKey {
                        plugin,
                        request_id: 0,
                    },
                    event: Event::Button {
                        name: ButtonName::East,
                        state: ButtonState::Pressed,
                    },
                    permissions: Vec::new(),
                },
            );
        }
        let fallback = vec![Action::Text {
            text: Text::new("fallback".into()).unwrap(),
        }];
        let (state, actions, rejection) = plugin::step(state, Input::Exited { plugin: 0 });
        assert_eq!(actions, fallback);
        assert!(rejection.is_none());
        assert_eq!(state.plugin_invoke_words(), ["word-1"]);
        let (state, late, _) = plugin::step(
            state,
            Input::Reply {
                key: RequestKey {
                    plugin: 0,
                    request_id: 0,
                },
                result: Ok(Vec::new()),
            },
        );
        assert!(late.is_empty(), "the exited request cannot fall back twice");
        let (state, surviving, _) = plugin::step(
            state,
            Input::Reply {
                key: RequestKey {
                    plugin: 1,
                    request_id: 0,
                },
                result: Ok(Vec::new()),
            },
        );
        assert_eq!(surviving, fallback, "the other request remains pending");
        assert_eq!(state.plugin_invoke_words(), ["word-1"]);
        let (_, duplicate, _) = plugin::step(state, Input::Exited { plugin: 0 });
        assert!(duplicate.is_empty());
    }

    proptest! {
        #[test]
        fn exit_clears_exactly_the_exiting_plugins_words(claims in claims(), dead in 0..PLUGINS) {
            let mut state = State::default();
            for (plugin, words) in &claims {
                state.set_plugin_invoke_words(*plugin, words.clone());
            }
            let (state, _, _) = plugin::step(state, Input::Exited { plugin: dead });
            let expected: Vec<String> = claims
                .into_iter()
                .filter(|(plugin, _)| *plugin != dead)
                .flat_map(|(_, words)| words)
                .collect();
            prop_assert_eq!(state.plugin_invoke_words(), expected);
        }

        #[test]
        fn a_second_exit_changes_nothing(claims in claims(), dead in 0..PLUGINS) {
            let mut state = State::default();
            for (plugin, words) in claims {
                state.set_plugin_invoke_words(plugin, words);
            }
            let (once, _, _) = plugin::step(state, Input::Exited { plugin: dead });
            let (twice, actions, _) = plugin::step(once.clone(), Input::Exited { plugin: dead });
            prop_assert!(actions.is_empty());
            prop_assert_eq!(once.plugin_invoke_words(), twice.plugin_invoke_words());
        }
    }
}
