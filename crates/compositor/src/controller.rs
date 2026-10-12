//! The controller edge: finds the pad, grabs it, and parses its events into protocol events.

use std::{
    io,
    os::fd::{AsFd, BorrowedFd},
};

use evdev::{Device, EventSummary, KeyCode};
use tehuti_protocol::{ButtonName, ButtonState, Event};
use tracing::warn;

use crate::input::InputSource;

/// Opens and grabs the first pad with a south face button that no other program holds,
/// so only Tehuti sees its input. Pads that fail to grab are logged and skipped.
#[must_use]
pub fn open_grabbed() -> Option<EvdevPad> {
    evdev::enumerate()
        .map(|(_, device)| device)
        .filter(|device| {
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::BTN_SOUTH))
        })
        .find_map(|mut device| match grab_nonblocking(&mut device) {
            Ok(()) => Some(EvdevPad(device)),
            Err(error) => {
                warn!(%error, name = device.name().unwrap_or("unnamed"), "controller grab failed; skipping it");
                None
            }
        })
}

fn grab_nonblocking(device: &mut Device) -> io::Result<()> {
    device.grab()?;
    device.set_nonblocking(true)
}

/// A grabbed kernel evdev controller, such as an Xbox pad on xpad.
#[derive(Debug)]
pub struct EvdevPad(Device);

impl EvdevPad {
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.name().unwrap_or("unnamed")
    }
}

impl AsFd for EvdevPad {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl InputSource for EvdevPad {
    fn release(&mut self) -> io::Result<()> {
        self.0.ungrab()
    }

    fn drain(&mut self) -> io::Result<Vec<Event>> {
        drain_device(&mut self.0)
    }
}

/// Reads every pending event from `device`. Stops at `WouldBlock`.
fn drain_device(device: &mut Device) -> io::Result<Vec<Event>> {
    match device.fetch_events() {
        Ok(events) => Ok(events
            .filter_map(|event| {
                if let EventSummary::Key(_, code, value) = event.destructure() {
                    parse_button(code, value)
                } else {
                    None
                }
            })
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Parses an evdev key event. Autorepeat (value 2) and unknown codes give `None`.
const fn parse_button(code: KeyCode, value: i32) -> Option<Event> {
    let state = match value {
        1 => ButtonState::Pressed,
        0 => ButtonState::Released,
        _ => return None,
    };
    let name = match code {
        KeyCode::BTN_SOUTH => ButtonName::South,
        KeyCode::BTN_EAST => ButtonName::East,
        KeyCode::BTN_NORTH => ButtonName::North,
        KeyCode::BTN_WEST => ButtonName::West,
        KeyCode::BTN_TL => ButtonName::LeftBumper,
        KeyCode::BTN_TR => ButtonName::RightBumper,
        KeyCode::BTN_SELECT => ButtonName::Select,
        KeyCode::BTN_START => ButtonName::Start,
        KeyCode::BTN_MODE => ButtonName::Guide,
        KeyCode::BTN_THUMBL => ButtonName::LeftStick,
        KeyCode::BTN_THUMBR => ButtonName::RightStick,
        KeyCode::BTN_DPAD_UP => ButtonName::DpadUp,
        KeyCode::BTN_DPAD_DOWN => ButtonName::DpadDown,
        KeyCode::BTN_DPAD_LEFT => ButtonName::DpadLeft,
        KeyCode::BTN_DPAD_RIGHT => ButtonName::DpadRight,
        _ => return None,
    };
    Some(Event::Button { name, state })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn south_button_press_is_a_pressed() {
        assert_eq!(
            parse_button(KeyCode::BTN_SOUTH, 1),
            Some(Event::Button {
                name: ButtonName::South,
                state: ButtonState::Pressed
            }),
            "xpad reports A as BTN_SOUTH"
        );
    }

    #[test]
    fn autorepeat_and_unknown_codes_are_dropped() {
        assert_eq!(parse_button(KeyCode::BTN_SOUTH, 2), None, "autorepeat");
        assert_eq!(parse_button(KeyCode::KEY_A, 1), None, "keyboard key");
    }
}
