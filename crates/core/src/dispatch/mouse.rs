use super::Effect;
use core::time::Duration;
use tehuti_protocol::{ButtonName, ButtonState, Event};

const VIEW_DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
}

/// Stick deflections after the device deadzone, in thousandths of full travel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sticks {
    pub left: (i16, i16),
    pub right: (i16, i16),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct State {
    active: bool,
    hold: Option<Duration>,
    toggled: bool,
    previous_tap: Option<Duration>,
    clicks: [bool; 2],
    pub(super) sticks: Sticks,
}

impl State {
    pub(super) fn button(&mut self, event: &Event, effects: &mut Vec<Effect>) -> bool {
        let Event::Button { name, state } = event else {
            return false;
        };
        if *name == ButtonName::Select {
            self.view_button(*state, effects);
            return true;
        }
        if !self.active {
            return false;
        }
        let click = match name {
            ButtonName::South => Some((0, PointerButton::Left)),
            ButtonName::East => Some((1, PointerButton::Right)),
            ButtonName::North
            | ButtonName::West
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick
            | ButtonName::DpadUp
            | ButtonName::DpadDown
            | ButtonName::DpadLeft
            | ButtonName::DpadRight => None,
        };
        if let Some((index, button)) = click {
            let pressed = *state == ButtonState::Pressed;
            if let Some(held) = self.clicks.get_mut(index)
                && *held != pressed
            {
                *held = pressed;
                effects.push(Effect::Click { button, pressed });
            }
        }
        true
    }

    fn view_button(&mut self, state: ButtonState, effects: &mut Vec<Effect>) {
        match state {
            ButtonState::Pressed if self.hold.is_none() => {
                self.hold = Some(Duration::ZERO);
                self.toggled = self
                    .previous_tap
                    .take()
                    .is_some_and(|elapsed| elapsed <= VIEW_DOUBLE_TAP_WINDOW);
                if self.toggled {
                    self.release(effects);
                    self.active = !self.active;
                    effects.push(Effect::MouseMode(self.active));
                }
            }
            ButtonState::Released => {
                if let Some(held) = self.hold.take() {
                    self.previous_tap = if self.toggled { None } else { Some(held) };
                }
            }
            ButtonState::Pressed => {}
        }
    }

    pub(super) fn tick(&mut self, elapsed: Duration, effects: &mut Vec<Effect>) -> bool {
        let mut single_tap = false;
        if let Some(hold) = &mut self.hold {
            *hold = hold.saturating_add(elapsed);
        }
        if let Some(previous) = &mut self.previous_tap {
            *previous = previous.saturating_add(elapsed);
            if *previous > VIEW_DOUBLE_TAP_WINDOW {
                self.previous_tap = None;
                single_tap = !self.active;
            }
        }
        if self.active {
            let millis = i32::try_from(elapsed.as_millis().min(100)).unwrap_or(100);
            let motion = |axis: i16| {
                let axis = i32::from(axis).clamp(-1000, 1000);
                axis.saturating_mul(axis.abs()).saturating_mul(millis)
            };
            let (x, y) = self.sticks.left;
            if x != 0 || y != 0 {
                effects.push(Effect::MovePointer {
                    x: motion(x),
                    y: motion(y),
                });
            }
            let (scroll_x, scroll_y) = self.sticks.right;
            if scroll_x != 0 || scroll_y != 0 {
                effects.push(Effect::Scroll {
                    x: i32::from(scroll_x)
                        .clamp(-1000, 1000)
                        .saturating_mul(millis)
                        .saturating_mul(250),
                    y: i32::from(scroll_y)
                        .clamp(-1000, 1000)
                        .saturating_mul(millis)
                        .saturating_mul(250),
                });
            }
        }
        single_tap
    }

    fn release(&mut self, effects: &mut Vec<Effect>) {
        for (held, button) in self
            .clicks
            .iter_mut()
            .zip([PointerButton::Left, PointerButton::Right])
        {
            if core::mem::take(held) {
                effects.push(Effect::Click {
                    button,
                    pressed: false,
                });
            }
        }
    }

    pub(super) fn disconnect(&mut self, effects: &mut Vec<Effect>) {
        self.release(effects);
        if self.active {
            effects.push(Effect::MouseMode(false));
        }
        *self = Self::default();
    }
}
