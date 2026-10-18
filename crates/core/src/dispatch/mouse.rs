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
    full_hold: Duration,
    context_repeat: Duration,
    hold: Option<Duration>,
    toggled: bool,
    previous_tap: Option<Duration>,
    clicks: [bool; 2],
    pub(super) sticks: Sticks,
}

impl State {
    pub(super) fn set_sticks(&mut self, sticks: Sticks) {
        if sticks.left != self.sticks.left {
            self.full_hold = Duration::ZERO;
        }
        self.sticks = sticks;
    }

    pub(super) fn short_release(&mut self, event: &Event) -> bool {
        if matches!(
            event,
            Event::Button {
                name: ButtonName::Select,
                state: ButtonState::Released
            }
        ) && let Some(held) = self.hold.take()
        {
            self.previous_tap = if !self.toggled && held <= VIEW_DOUBLE_TAP_WINDOW {
                Some(held)
            } else {
                None
            };
            return !self.toggled && !self.active;
        }
        false
    }

    pub(super) fn button(&mut self, event: &Event, effects: &mut Vec<Effect>) -> bool {
        let Event::Button { name, state } = event else {
            return false;
        };
        if *name == ButtonName::Select {
            self.view_button(*state, effects);
            return true;
        }
        if !self.active || matches!(name, ButtonName::Record | ButtonName::Guide | ButtonName::Start) {
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
            | ButtonName::GuideTap
            | ButtonName::KeyboardPtt
            | ButtonName::Paddle1
            | ButtonName::Paddle2
            | ButtonName::Paddle3
            | ButtonName::Paddle4
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
                self.hold = None;
            }
            ButtonState::Pressed => {}
        }
    }

    pub(super) fn context_tick(
        &mut self,
        context: &tehuti_protocol::Context,
        elapsed: Duration,
        effects: &mut Vec<Effect>,
    ) {
        let sticks = core::mem::take(&mut self.sticks);
        self.tick(elapsed, effects);
        self.sticks = sticks;
        self.context_repeat = self.context_repeat.saturating_add(elapsed);
        if self.context_repeat >= Duration::from_millis(180) {
            self.context_repeat = Duration::ZERO;
            effects.extend(
                context
                    .stick_actions(self.sticks.left, self.sticks.right)
                    .into_iter()
                    .map(Effect::Apply),
            );
        }
    }

    pub(super) fn tick(&mut self, elapsed: Duration, effects: &mut Vec<Effect>) {
        if let Some(hold) = &mut self.hold {
            *hold = hold.saturating_add(elapsed);
        }
        if let Some(previous) = &mut self.previous_tap {
            *previous = previous.saturating_add(elapsed);
            if *previous > VIEW_DOUBLE_TAP_WINDOW {
                self.previous_tap = None;
            }
        }
        let nanos = i64::try_from(elapsed.as_nanos().min(100_000_000)).unwrap_or(100_000_000);
        let motion = |axis: i16| {
            let axis = i64::from(axis).clamp(-1000, 1000);
            i32::try_from(
                axis.saturating_mul(axis.abs())
                    .saturating_mul(nanos)
                    .saturating_div(1_000_000),
            )
            .unwrap_or(0)
        };
        if self.active {
            let (x, y) = self.sticks.left;
            if x != 0 || y != 0 {
                let full = x.unsigned_abs().max(y.unsigned_abs()) >= 1000;
                let start = self.full_hold;
                self.full_hold = if full {
                    start.saturating_add(Duration::from_nanos(u64::try_from(nanos).unwrap_or_default()))
                } else {
                    Duration::ZERO
                };
                let boost = if full {
                    integrated_boost(self.full_hold).saturating_sub(integrated_boost(start))
                } else {
                    i128::from(nanos).saturating_mul(920).saturating_div(1000)
                };
                let pointer = |axis: i16| {
                    let outer = i128::from(axis).abs().clamp(400, 1000).saturating_sub(400);
                    let extra = outer
                        .saturating_mul(outer)
                        .saturating_mul(boost)
                        .saturating_div(360_000)
                        .saturating_mul(i128::from(axis.signum()));
                    i32::try_from(i128::from(motion(axis)).saturating_add(extra)).unwrap_or_default()
                };
                effects.push(Effect::MovePointer {
                    x: pointer(x),
                    y: pointer(y),
                });
            }
        }
        let (scroll_x, scroll_y) = self.sticks.right;
        let scroll_x = if self.active { scroll_x } else { 0 };
        if scroll_x != 0 || scroll_y != 0 {
            effects.push(Effect::Scroll {
                x: motion(scroll_x).saturating_div(4).saturating_mul(3),
                y: motion(scroll_y).saturating_div(4).saturating_mul(3),
            });
        }
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

fn integrated_boost(held: Duration) -> i128 {
    let nanos = i128::try_from(held.as_nanos()).unwrap_or(i128::MAX);
    let ramp = nanos.min(500_000_000);
    nanos
        .saturating_mul(920)
        .saturating_div(1000)
        .saturating_add(
            ramp.saturating_mul(ramp)
                .saturating_mul(1920)
                .saturating_div(1_000_000_000_000),
        )
        .saturating_add(
            nanos
                .saturating_sub(ramp)
                .saturating_mul(1920)
                .saturating_div(1000),
        )
}
