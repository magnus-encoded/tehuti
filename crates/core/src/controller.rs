//! Pure controller session transitions and balanced key taps.

use tehuti_protocol::{ButtonName, ButtonState, Event};

#[derive(Debug)]
pub enum State {
    Connected { held: Vec<ButtonName> },
    Disconnected,
}

impl Default for State {
    fn default() -> Self {
        Self::Connected { held: Vec::new() }
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
            let old = core::mem::replace(&mut state, State::Disconnected);
            match old {
                State::Connected { held } => held
                    .into_iter()
                    .map(|name| Event::Button {
                        name,
                        state: ButtonState::Released,
                    })
                    .collect(),
                State::Disconnected => Vec::new(),
            }
        }
        Input::Reconnected => {
            if matches!(state, State::Disconnected) {
                state = State::default();
            }
            Vec::new()
        }
        Input::Event(Event::Button {
            name,
            state: button_state,
        }) => {
            let State::Connected { held } = &mut state else {
                return (state, Vec::new());
            };
            match button_state {
                ButtonState::Pressed if !held.contains(&name) => held.push(name),
                ButtonState::Released if held.contains(&name) => held.retain(|held| *held != name),
                ButtonState::Pressed | ButtonState::Released => return (state, Vec::new()),
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
