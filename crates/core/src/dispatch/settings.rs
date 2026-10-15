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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Home,
    Display,
    Wallpaper,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phrases {
    settings: String,
    unlock_wallpapers: String,
}

impl Default for Phrases {
    fn default() -> Self {
        Self {
            settings: "pop the hood".into(),
            unlock_wallpapers: "birds of a feather".into(),
        }
    }
}

impl Phrases {
    /// # Errors
    /// Rejects empty phrases and phrases that normalize to the same words.
    pub fn new(settings: &str, unlock_wallpapers: &str) -> Result<Self, String> {
        let settings = tehuti_protocol::normalize_phrase(settings);
        let unlock_wallpapers = tehuti_protocol::normalize_phrase(unlock_wallpapers);
        if settings.is_empty() || unlock_wallpapers.is_empty() || settings == unlock_wallpapers {
            return Err("voice phrases must contain words and be distinct".into());
        }
        Ok(Self {
            settings,
            unlock_wallpapers,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub(super) phrases: Phrases,
    pub open: bool,
    pub view: View,
    pub section: usize,
    pub wallpapers: Vec<String>,
    pub wallpaper_locked: Vec<bool>,
    pub frequency_group: bool,
    pub wallpaper_current: Option<usize>,
    pub selected: usize,
    pub current: usize,
    pub modes: Vec<String>,
    pub native_size: Option<(u32, u32)>,
    pub remaining: Option<Duration>,
    previous: Option<usize>,
    status: Status,
    cancel_preview: bool,
}

impl Settings {
    #[must_use]
    pub fn sizes(&self) -> Vec<(u32, u32)> {
        let mut sizes = Vec::new();
        for mode in &self.modes {
            if let Some(size) = mode_size(mode).filter(|size| !sizes.contains(size)) {
                sizes.push(size);
            }
        }
        sizes
    }

    #[must_use]
    pub fn rates(&self) -> Vec<usize> {
        let size = self.modes.get(self.selected).and_then(|mode| mode_size(mode));
        self.modes
            .iter()
            .enumerate()
            .filter(|(_, mode)| size.is_some() && mode_size(mode) == size)
            .map(|(index, _)| index)
            .collect()
    }

    fn display_move(&mut self, name: ButtonName) {
        if name == ButtonName::DpadLeft {
            self.frequency_group = false;
        } else if name == ButtonName::DpadRight {
            self.frequency_group = true;
        } else {
            let choices = if self.frequency_group {
                self.rates()
            } else {
                self.sizes()
                    .iter()
                    .filter_map(|size| self.modes.iter().position(|mode| mode_size(mode) == Some(*size)))
                    .collect()
            };
            let selected_size = self.modes.get(self.selected).and_then(|mode| mode_size(mode));
            let position = choices
                .iter()
                .position(|index| {
                    if self.frequency_group {
                        *index == self.selected
                    } else {
                        self.modes.get(*index).and_then(|mode| mode_size(mode)) == selected_size
                    }
                })
                .unwrap_or_default();
            let next = if name == ButtonName::DpadUp {
                position.saturating_sub(1)
            } else {
                position.saturating_add(1).min(choices.len().saturating_sub(1))
            };
            if let Some(index) = choices.get(next) {
                let rate = self
                    .modes
                    .get(self.selected)
                    .and_then(|mode| mode.split_once('@'))
                    .map(|(_, rate)| rate);
                self.selected = if self.frequency_group {
                    *index
                } else {
                    let size = self.modes.get(*index).and_then(|mode| mode_size(mode));
                    self.modes
                        .iter()
                        .position(|mode| {
                            mode_size(mode) == size
                                && mode.split_once('@').map(|(_, candidate_rate)| candidate_rate) == rate
                        })
                        .unwrap_or(*index)
                };
            }
        }
    }

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

    pub(super) fn configure_wallpapers(&mut self, names: Vec<String>, current: Option<usize>) {
        self.wallpapers = names;
        self.wallpaper_current = current;
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
        if let Event::Speech { text } = event {
            let phrase = tehuti_protocol::normalize_phrase(text.as_str());
            if phrase == self.phrases.unlock_wallpapers {
                effects.push(Effect::UnlockWallpapers);
                return true;
            }
            if phrase == self.phrases.settings {
                self.toggle(effects);
                return true;
            }
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
                if self.view == View::Display {
                    self.view = View::Home;
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
        self.view = View::Home;
    }

    fn apply(&mut self, effects: &mut Vec<Effect>) {
        if self.remaining.take().is_some() {
            self.previous = None;
            effects.push(Effect::PersistMode(self.current));
        } else if self.view == View::Wallpaper {
            if self.selected < self.wallpapers.len()
                && !self.wallpaper_locked.get(self.selected).copied().unwrap_or(false)
            {
                self.wallpaper_current = Some(self.selected);
                effects.push(Effect::Wallpaper(self.selected));
            }
        } else if self.view == View::Home && self.section == 1 {
            self.view = View::Wallpaper;
            self.selected = self.wallpaper_current.unwrap_or_default();
        } else if self.view == View::Home {
            self.view = View::Display;
            self.selected = self.current;
        } else if self.selected != self.current && !self.modes.is_empty() {
            self.cancel_preview = false;
            self.previous = Some(self.current);
            self.status = Status::Switching;
            effects.push(Effect::SettingsMode(self.selected));
        }
    }

    fn move_selection(&mut self, name: ButtonName) {
        if self.view == View::Display {
            self.display_move(name);
            return;
        }
        let backwards = matches!(name, ButtonName::DpadUp | ButtonName::DpadLeft);
        if self.view == View::Home {
            self.section = usize::from(!backwards);
            return;
        }
        let distance = if matches!(name, ButtonName::DpadUp | ButtonName::DpadDown) {
            4
        } else {
            1
        };
        self.selected = if backwards {
            self.selected.saturating_sub(distance)
        } else {
            self.selected
                .saturating_add(distance)
                .min(self.wallpapers.len().saturating_sub(1))
        };
    }

    fn button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        match name {
            ButtonName::East => {
                if self.remaining.is_some() {
                    self.revert(effects);
                } else if self.view != View::Home {
                    self.view = View::Home;
                } else {
                    self.open = false;
                }
            }
            ButtonName::South => {
                self.apply(effects);
            }
            ButtonName::DpadUp | ButtonName::DpadDown | ButtonName::DpadLeft | ButtonName::DpadRight
                if self.remaining.is_none() =>
            {
                self.move_selection(name);
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

fn mode_size(mode: &str) -> Option<(u32, u32)> {
    let (size, _) = mode.split_once('@')?;
    let (width, height) = size.split_once('x')?;
    Some((
        width.parse().ok().filter(|value| *value > 0)?,
        height.parse().ok().filter(|value| *value > 0)?,
    ))
}
