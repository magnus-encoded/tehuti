#[cfg(test)]
mod new_launcher {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{
        State as CoreState,
        dispatch::{Effect, Input, State, step},
        unit_verbs::{DestinationKind, LauncherRequest},
    };
    use tehuti_protocol::{Action, Event, KeyName, Text, parse_context};

    fn drive(mut state: State, event: Event, plugin: bool) -> (State, Vec<Effect>) {
        let mut input = Input::Event {
            event,
            elapsed: Duration::ZERO,
        };
        let mut all = Vec::new();
        for _ in 0..32 {
            let (next, effects) = step(state, input);
            state = next;
            input = Input::Resume;
            for effect in effects {
                if matches!(effect, Effect::QueryPlugin(_)) {
                    input = Input::PluginActive(plugin);
                } else if matches!(effect, Effect::DispatchPlugin { .. }) {
                    input = Input::PluginRequest(None);
                } else if effect == Effect::ReadClock {
                    input = Input::ClockRead(Duration::ZERO);
                } else if matches!(effect, Effect::PollSpeech(_)) {
                    input = Input::SpeechPolled(vec![]);
                } else if effect == Effect::Supervise {
                    input = Input::Supervised(vec![]);
                }
                all.push(effect);
            }
            if state.busy() == 0 {
                break;
            }
        }
        (state, all)
    }

    fn world(contexts: &[&str], focus: Option<&str>) -> State {
        let contexts = contexts
            .iter()
            .map(|source| parse_context(source).unwrap_or_else(|error| panic!("{error}")))
            .collect();
        let state = State::new(CoreState::with_contexts(contexts));
        drive(
            state,
            Event::Focus {
                app_id: focus.map(Into::into),
                title: String::new(),
            },
            false,
        )
        .0
    }

    fn say(state: State, phrase: &str) -> Vec<Effect> {
        say_with(state, phrase, false)
    }

    fn say_with(state: State, phrase: &str, plugin: bool) -> Vec<Effect> {
        drive(
            state,
            Event::Speech {
                text: Text::new(phrase.into()).unwrap(),
            },
            plugin,
        )
        .1
    }

    #[test]
    fn a_plugin_that_handles_speech_keeps_new_tab() {
        let effects = say_with(world(&[], None), "new tab", true);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::OpenLauncher(_))),
            "{effects:?}"
        );
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::DispatchPlugin {
                    event: Event::Speech { .. },
                    ..
                }
            )),
            "{effects:?}"
        );
    }

    fn creation(query: &str) -> Effect {
        Effect::OpenLauncher(LauncherRequest {
            kind: DestinationKind::Creation,
            query: query.into(),
        })
    }

    const TERMINAL: &str = "app_id = 'terminal'\n[buttons]\n[speech]\n'new tab' = { key = 'enter' }";

    #[test]
    fn new_agent_with_nothing_focused_opens_creation_with_the_query() {
        let effects = say(world(&[], None), "New agent.");
        assert!(effects.contains(&creation("agent")), "{effects:?}");
    }

    #[test]
    fn focused_app_phrase_wins() {
        let effects = say(world(&[TERMINAL], Some("terminal")), "new tab");
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::OpenLauncher(_))),
            "{effects:?}"
        );
        assert!(
            effects.contains(&Effect::Apply(Action::Key {
                key: KeyName::Enter,
                mods: Vec::new()
            })),
            "{effects:?}"
        );
    }

    #[test]
    fn focused_app_without_the_phrase_gets_the_launcher() {
        let effects = say(world(&[TERMINAL], Some("terminal")), "new agent");
        assert!(effects.contains(&creation("agent")), "{effects:?}");
    }

    proptest! {
        #[test]
        fn any_unmapped_query_opens_creation(
            query in "[a-z]{3,8}( [a-z]{3,8}){0,2}",
            focused in any::<bool>(),
        ) {
            prop_assume!(!query.starts_with("tab"));
            let focus = focused.then_some("terminal");
            let effects = say(world(&[TERMINAL], focus), &format!("new {query}"));
            prop_assert!(effects.contains(&creation(&query)), "{effects:?}");
        }
    }
}
