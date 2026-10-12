//! The pure core step: state and one event in, state and actions out.

use tehuti_protocol::{Action, ButtonName, ButtonState, Event, KeyName};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State;

impl State {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Advances the core by one event. Performs no I/O.
#[must_use]
pub fn step(state: State, event: &Event) -> (State, Vec<Action>) {
    let actions = global_layer(event).into_iter().collect();
    (state, actions)
}

/// The fixed global mapping. Events it does not map reach no client.
const fn global_layer(event: &Event) -> Option<Action> {
    match *event {
        Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        } => Some(Action::Key { key: KeyName::Enter }),
        Event::Button { .. } => None,
    }
}
