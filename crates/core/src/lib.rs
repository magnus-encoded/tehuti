//! The pure core step: state and one event in, state and actions out.

use tehuti_protocol::{Action, ButtonName, ButtonState, Context, Event, KeyName};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    contexts: Vec<Context>,
    focused_app_id: Option<String>,
}

impl State {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn with_contexts(contexts: Vec<Context>) -> Self {
        Self {
            contexts,
            focused_app_id: None,
        }
    }
}

/// Advances the core by one event. Performs no I/O.
#[must_use]
pub fn step(mut state: State, event: &Event) -> (State, Vec<Action>) {
    if let Event::Focus { app_id, .. } = event {
        state.focused_app_id.clone_from(app_id);
    }
    let actions = global_layer(event)
        .or_else(|| active_context(&state, event))
        .or_else(|| fallback(event))
        .into_iter()
        .collect();
    (state, actions)
}

/// The fixed global mapping takes priority over contexts.
const fn global_layer(event: &Event) -> Option<Action> {
    match *event {
        Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        } => Some(Action::Key { key: KeyName::Enter }),
        Event::Button { .. } | Event::Focus { .. } => None,
    }
}

fn active_context(state: &State, event: &Event) -> Option<Action> {
    let Event::Button {
        name,
        state: ButtonState::Pressed,
    } = event
    else {
        return None;
    };
    state
        .contexts
        .iter()
        .find(|context| Some(context.app_id()) == state.focused_app_id.as_deref())
        .and_then(|context| context.key_for(*name))
        .map(|key| Action::Key { key })
}

const fn fallback(_event: &Event) -> Option<Action> {
    None
}
