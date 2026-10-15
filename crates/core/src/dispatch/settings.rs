//! Controller-owned display settings and timed mode confirmation.

use super::Effect;
use core::time::Duration;
use tehuti_protocol::{ButtonName, ButtonState, Event};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Status {
    #[default]
    Ready,
    Switching,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub open: bool,
    pub display: bool,
    pub selected: usize,
    pub current: usize,
    pub modes: Vec<String>,
    pub remaining: Option<Duration>,
    previous: Option<usize>,
    status: Status,
    cancel_preview: bool,
}

impl Settings {
    #[must_use]
    pub const fn button_hint(&self) -> &'static str {
        if self.remaining.is_some() {
            "A: KEEP THIS MODE"
        } else {
            "D-PAD: MOVE   A: APPLY   B: BACK"
        }
    }

    #[must_use]
    pub fn failed(&self) -> bool {
        self.status == Status::Failed
    }

    pub(super) fn configure(&mut self, modes: Vec<String>, current: usize) {
        if current < modes.len() {
            self.modes = modes;
            self.current = current;
            self.selected = current;
        }
    }

    pub(super) fn applied(&mut self, current: Option<usize>, effects: &mut Vec<Effect>) {
        self.status = if current.is_none() {
            Status::Failed
        } else {
            Status::Ready
        };
        if let Some(current) = current.filter(|index| *index < self.modes.len()) {
            self.current = current;
            if self.cancel_preview {
                self.revert(effects);
            } else if self.previous.is_some() {
                self.remaining = Some(Duration::from_secs(15));
            }
        } else {
            self.previous = None;
            self.remaining = None;
        }
    }

    pub(super) fn tick(&mut self, elapsed: Duration, effects: &mut Vec<Effect>) {
        if let Some(remaining) = &mut self.remaining {
            *remaining = remaining.saturating_sub(elapsed);
            if remaining.is_zero() {
                self.revert(effects);
            }
        }
    }

    fn revert(&mut self, effects: &mut Vec<Effect>) {
        self.remaining = None;
        if let Some(previous) = self.previous.take() {
            self.status = Status::Switching;
            effects.push(Effect::SettingsMode(previous));
        }
    }

    pub(super) fn event(&mut self, event: &Event, effects: &mut Vec<Effect>) -> bool {
        if let Event::Speech { text } = event
            && text
                .as_str()
                .trim()
                .trim_end_matches('.')
                .eq_ignore_ascii_case("pop the hood")
        {
            self.toggle(effects);
            return true;
        }
        if !self.open {
            return false;
        }
        let Event::Button { name, state } = event else {
            return false;
        };
        if matches!(name, ButtonName::Guide | ButtonName::Start | ButtonName::Record) {
            return false;
        }
        if *state == ButtonState::Released {
            return true;
        }
        if self.status == Status::Switching {
            if *name == ButtonName::East {
                self.cancel_preview = true;
                if self.display {
                    self.display = false;
                } else {
                    self.open = false;
                }
            }
            return true;
        }
        self.button(*name, effects);
        true
    }

    fn toggle(&mut self, effects: &mut Vec<Effect>) {
        if self.status == Status::Switching {
            self.cancel_preview = true;
        } else {
            self.revert(effects);
        }
        self.open = !self.open;
        self.display = false;
    }

    fn button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        match name {
            ButtonName::East => {
                if self.remaining.is_some() {
                    self.revert(effects);
                } else if self.display {
                    self.display = false;
                } else {
                    self.open = false;
                }
            }
            ButtonName::South => {
                if self.remaining.take().is_some() {
                    self.previous = None;
                    effects.push(Effect::PersistMode(self.current));
                } else if !self.display {
                    self.display = true;
                    self.selected = self.current;
                } else if self.selected != self.current && !self.modes.is_empty() {
                    self.cancel_preview = false;
                    self.previous = Some(self.current);
                    self.status = Status::Switching;
                    effects.push(Effect::SettingsMode(self.selected));
                }
            }
            ButtonName::DpadUp | ButtonName::DpadLeft if self.remaining.is_none() => {
                self.selected = self.selected.saturating_sub(1);
            }
            ButtonName::DpadDown | ButtonName::DpadRight if self.remaining.is_none() => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(self.modes.len().saturating_sub(1));
            }
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
            | ButtonName::DpadRight => {}
        }
    }
}
