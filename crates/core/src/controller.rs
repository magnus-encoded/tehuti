//! Pure controller session transitions and balanced key taps.

use tehuti_protocol::{ButtonName, ButtonState, Event};

#[derive(Debug)]
pub struct State {
    held: Vec<ButtonName>,
    connected: bool,
    discard_unpaired_releases: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            connected: true,
            discard_unpaired_releases: false,
        }
    }
}

#[derive(Debug)]
pub enum Input {
    Event(Event),
    Disconnected,
    Reconnected,
}

/// Returns the events belonging to this controller session.
#[must_use]
pub fn step(mut state: State, input: Input) -> (State, Vec<Event>) {
    let events = match input {
        Input::Disconnected => {
            state.connected = false;
            state.discard_unpaired_releases = true;
            state
                .held
                .drain(..)
                .map(|name| Event::Button {
                    name,
                    state: ButtonState::Released,
                })
                .collect()
        }
        Input::Reconnected => {
            state.connected = true;
            Vec::new()
        }
        Input::Event(Event::Button {
            name,
            state: button_state,
        }) => {
            if !state.connected {
                return (state, Vec::new());
            }
            match button_state {
                ButtonState::Pressed if !state.held.contains(&name) => {
                    state.held.push(name);
                }
                ButtonState::Released if state.held.contains(&name) => {
                    state.held.retain(|held| *held != name);
                }
                ButtonState::Released if state.discard_unpaired_releases => return (state, Vec::new()),
                ButtonState::Pressed | ButtonState::Released => {}
            }
            vec![Event::Button {
                name,
                state: button_state,
            }]
        }
        Input::Event(event) => vec![event],
    };
    (state, events)
}

/// A key action is one atomic tap, completed before another event is handled.
#[must_use]
pub const fn key_tap() -> [ButtonState; 2] {
    [ButtonState::Pressed, ButtonState::Released]
}
