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

    proptest! {
        #[test]
        fn exit_clears_exactly_the_exiting_plugins_words(claims in claims(), dead in 0..PLUGINS) {
            let mut state = State::default();
            state.set_plugin_invoke_words(claims.clone());
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
            state.set_plugin_invoke_words(claims);
            let (once, _, _) = plugin::step(state, Input::Exited { plugin: dead });
            let (twice, actions, _) = plugin::step(once.clone(), Input::Exited { plugin: dead });
            prop_assert!(actions.is_empty());
            prop_assert_eq!(once.plugin_invoke_words(), twice.plugin_invoke_words());
        }
    }
}
