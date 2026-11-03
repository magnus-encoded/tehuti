//! A context with a window title wins over one matching the application id alone.
#[cfg(test)]
mod context_title {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Event, parse_context};

    const APP: &str = "steam_proton";

    fn plain() -> tehuti_protocol::Context {
        parse_context(&format!("app_id = '{APP}'\n[buttons]\n[dim]\nfade_ms = 1\n")).unwrap()
    }

    fn titled(title: &str) -> tehuti_protocol::Context {
        parse_context(&format!(
            "app_id = '{APP}'\ntitle = {title:?}\n[buttons]\n[dim]\nfade_ms = 2\n"
        ))
        .unwrap()
    }

    fn focus(state: State, title: &str) -> State {
        step(
            state,
            &Event::Focus {
                app_id: Some(APP.into()),
                title: title.into(),
            },
        )
        .0
    }

    fn fade(state: &State) -> Duration {
        state.dim_config().2
    }

    proptest! {
        #[test]
        fn the_titled_context_wins_in_any_order_and_title_changes_reevaluate(
            title in "[A-Za-z][A-Za-z ]{0,19}", other in "[A-Za-z ]{0,20}", titled_first in any::<bool>(),
        ) {
            prop_assume!(other != title);
            let contexts = if titled_first { vec![titled(&title), plain()] } else { vec![plain(), titled(&title)] };
            let state = focus(State::with_contexts(contexts), &other);
            prop_assert_eq!(fade(&state), Duration::from_millis(1));
            let state = focus(state, &title);
            prop_assert_eq!(fade(&state), Duration::from_millis(2));
            let state = focus(state, &other);
            prop_assert_eq!(fade(&state), Duration::from_millis(1));
        }

        #[test]
        fn a_titled_context_alone_never_matches_other_titles(title in "[A-Za-z]{1,20}", other in "[A-Za-z]{0,20}") {
            prop_assume!(other != title);
            let state = focus(State::with_contexts(vec![titled(&title)]), &other);
            prop_assert_eq!(fade(&state), Duration::from_millis(100));
        }
    }

    #[test]
    fn titles_match_exactly_and_must_not_be_blank() {
        let state = focus(
            State::with_contexts(vec![plain(), titled("World of Warcraft")]),
            "BattleNet - Wine Desktop",
        );
        assert_eq!(fade(&state), Duration::from_millis(1));
        let state = focus(state, "World of Warcraft Classic");
        assert_eq!(fade(&state), Duration::from_millis(1));
        assert!(parse_context("app_id = 'a'\ntitle = ' '\n[buttons]\n").is_err());
        assert!(parse_context("app_id = '*'\ntitle = 'x'\n[buttons]\n").is_err());
    }
}
