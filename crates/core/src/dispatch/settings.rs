//! Controller-owned settings pages and timed confirmation of risky changes.

use super::{
    Effect,
    input_settings::{ControllerChoice, InputRow, InputSettings, cycle},
};
use core::time::Duration;
use tehuti_protocol::{ButtonName, ButtonState, Event};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Status {
    #[default]
    Ready,
    Switching,
    Failed,
}

/// The page the settings view shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    /// The list of categories.
    #[default]
    Categories,
    Display,
    Input,
    Wallpaper,
}

/// A top-level entry of the settings view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Category {
    #[default]
    Display,
    Input,
    Wallpaper,
}

impl Category {
    pub const ALL: [Self; 3] = [Self::Display, Self::Input, Self::Wallpaper];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub open: bool,
    pub page: Page,
    /// The highlighted category on the categories page.
    pub category: Category,
    pub input: InputSettings,
    pub selected: usize,
    pub current: usize,
    pub modes: Vec<String>,
    pub wallpapers: Vec<String>,
    pub wallpaper_locked: Vec<bool>,
    pub wallpaper_selected: usize,
    pub wallpaper_current: usize,
    pub scale: core::num::NonZeroU32,
    pub remaining: Option<Duration>,
    previous: Option<usize>,
    previous_controller: Option<ControllerChoice>,
    status: Status,
    cancel_preview: bool,
    south: Option<ButtonState>,
    held: Vec<ButtonName>,
}

impl Settings {
    #[must_use]
    pub const fn button_hint(&self) -> &'static str {
        if self.remaining.is_some() {
            if self.previous_controller.is_some() {
                "A ON THE NEW PAD: KEEP IT"
            } else {
                "A: KEEP THIS MODE"
            }
        } else {
            match self.page {
                Page::Categories => "D-PAD: MOVE   A: OPEN   B: CLOSE",
                Page::Display => "D-PAD: MOVE   A: APPLY   B: BACK",
                Page::Input => "UP / DOWN: MOVE   LEFT / RIGHT: CHANGE   B: BACK",
                Page::Wallpaper => "D-PAD: PREVIEW   A: SAVE   B: CANCEL",
            }
        }
    }

    /// Sets the connected pad names shown on the Input page.
    pub(super) fn configure_controllers(&mut self, names: Vec<String>) {
        self.input.controllers = names;
    }

    pub(super) fn configure_microphones(&mut self, microphones: Vec<super::Microphone>) {
        self.input.microphones = microphones;
    }

    /// Restores saved choices; a pending controller confirmation keeps its own state.
    pub(super) fn restore_input(
        &mut self,
        controller: ControllerChoice,
        microphone: super::MicrophoneChoice,
    ) {
        if self.previous_controller.is_none() {
            self.input.controller = controller;
        }
        self.input.microphone = microphone;
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
        if let Some(previous) = self.previous_controller.take() {
            self.input.controller = previous.clone();
            effects.push(Effect::SettingsController(previous));
        }
    }

    fn back(&mut self, effects: &mut Vec<Effect>) {
        self.revert(effects);
        if self.page == Page::Wallpaper && self.wallpaper_selected != self.wallpaper_current {
            self.wallpaper_selected = self.wallpaper_current;
            effects.push(Effect::WallpaperPreview(self.wallpaper_current));
        }
        self.page = Page::Categories;
    }

    pub(super) fn close(&mut self, effects: &mut Vec<Effect>) {
        if self.status == Status::Switching {
            self.cancel_preview = true;
        } else {
            self.revert(effects);
        }
        self.open = false;
        if self.page == Page::Wallpaper && self.wallpaper_selected != self.wallpaper_current {
            self.wallpaper_selected = self.wallpaper_current;
            effects.push(Effect::WallpaperPreview(self.wallpaper_current));
        }
        self.page = Page::Categories;
    }

    pub(super) fn event(&mut self, event: &Event, effects: &mut Vec<Effect>) -> bool {
        if let Event::Speech { text } = event
            && text
                .as_str()
                .trim()
                .trim_end_matches('.')
                .eq_ignore_ascii_case("birds of a feather")
        {
            effects.push(Effect::UnlockWallpapers);
            return true;
        }
        if let Event::Speech { text } = event
            && is_open_phrase(text.as_str())
        {
            self.toggle(effects);
            return true;
        }
        let stale_south = if let Event::Button {
            name: ButtonName::South,
            state,
        } = event
        {
            let previous = self.south.replace(*state);
            previous == Some(ButtonState::Pressed) && *state == ButtonState::Pressed
        } else {
            false
        };
        let Event::Button { name, state } = event else {
            return false;
        };
        if matches!(name, ButtonName::Guide | ButtonName::Start | ButtonName::Record) {
            return false;
        }
        let was_held = self.held.contains(name);
        if *state == ButtonState::Released {
            self.held.retain(|held| held != name);
        } else if self.open && !was_held {
            self.held.push(*name);
        }
        if !self.open {
            return was_held;
        }
        if *state == ButtonState::Released || stale_south {
            return true;
        }
        if self.status == Status::Switching {
            if *name == ButtonName::East {
                self.cancel_preview = true;
                self.open = false;
                self.page = Page::Categories;
            }
            return true;
        }
        self.button(*name, effects);
        true
    }

    fn toggle(&mut self, effects: &mut Vec<Effect>) {
        if self.open && self.page == Page::Wallpaper && self.wallpaper_selected != self.wallpaper_current {
            self.wallpaper_selected = self.wallpaper_current;
            effects.push(Effect::WallpaperPreview(self.wallpaper_current));
        }
        if self.status == Status::Switching {
            self.cancel_preview = true;
        } else {
            self.revert(effects);
        }
        self.open = !self.open;
        self.page = Page::Categories;
    }

    fn button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        match self.page {
            Page::Categories => self.categories_button(name, effects),
            Page::Display => self.display_button(name, effects),
            Page::Input => self.input_button(name, effects),
            Page::Wallpaper => self.wallpaper_button(name, effects),
        }
    }

    fn categories_button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        match name {
            ButtonName::East => self.close(effects),
            ButtonName::DpadUp | ButtonName::DpadLeft => {
                self.category = Category::ALL
                    .get(self.category_index().saturating_sub(1))
                    .copied()
                    .unwrap_or(Category::Display);
            }
            ButtonName::DpadDown | ButtonName::DpadRight => {
                self.category = Category::ALL
                    .get(self.category_index().saturating_add(1))
                    .copied()
                    .unwrap_or(Category::Wallpaper);
            }
            ButtonName::South => match self.category {
                Category::Display => {
                    self.page = Page::Display;
                    self.selected = self.current;
                }
                Category::Input => {
                    self.page = Page::Input;
                    self.input.row = InputRow::Controller;
                    effects.push(Effect::ListMicrophones);
                }
                Category::Wallpaper => {
                    self.page = Page::Wallpaper;
                    self.wallpaper_selected = self.wallpaper_current;
                }
            },
            ButtonName::North
            | ButtonName::West
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick => {}
        }
    }

    fn category_index(&self) -> usize {
        Category::ALL
            .iter()
            .position(|category| *category == self.category)
            .unwrap_or(0)
    }

    pub(super) fn configure_wallpapers(&mut self, names: Vec<String>, locked: Vec<bool>, current: usize) {
        self.wallpapers = names;
        self.wallpaper_locked = locked;
        self.wallpaper_current = current.min(self.wallpapers.len().saturating_sub(1));
        self.wallpaper_selected = self.wallpaper_current;
    }

    pub(super) fn wallpaper_saved(&mut self, index: usize) {
        if index < self.wallpapers.len() && !self.wallpaper_locked.get(index).copied().unwrap_or(true) {
            self.wallpaper_current = index;
        }
    }

    fn wallpaper_button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        match name {
            ButtonName::East => self.back(effects),
            ButtonName::South if !self.wallpapers.is_empty() => {
                effects.push(Effect::WallpaperSave(self.wallpaper_selected));
            }
            ButtonName::DpadUp | ButtonName::DpadLeft => {
                if let Some(index) = (0..self.wallpaper_selected)
                    .rev()
                    .find(|index| !self.wallpaper_locked.get(*index).copied().unwrap_or(true))
                {
                    self.wallpaper_selected = index;
                    effects.push(Effect::WallpaperPreview(index));
                }
            }
            ButtonName::DpadDown | ButtonName::DpadRight if !self.wallpapers.is_empty() => {
                if let Some(index) = (self.wallpaper_selected.saturating_add(1)..self.wallpapers.len())
                    .find(|index| !self.wallpaper_locked.get(*index).copied().unwrap_or(true))
                {
                    self.wallpaper_selected = index;
                    effects.push(Effect::WallpaperPreview(index));
                }
            }
            ButtonName::South
            | ButtonName::North
            | ButtonName::West
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick
            | ButtonName::DpadDown
            | ButtonName::DpadRight => {}
        }
    }

    fn input_button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        if self.remaining.is_some() {
            match name {
                ButtonName::South => {
                    self.remaining = None;
                    self.previous_controller = None;
                    effects.push(Effect::PersistController(self.input.controller.clone()));
                }
                ButtonName::East => self.back(effects),
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
            return;
        }
        let forward = match name {
            ButtonName::East => return self.back(effects),
            ButtonName::DpadUp => {
                self.input.row = InputRow::Controller;
                return;
            }
            ButtonName::DpadDown => {
                self.input.row = InputRow::Microphone;
                return;
            }
            ButtonName::DpadRight | ButtonName::South => true,
            ButtonName::DpadLeft => false,
            ButtonName::North
            | ButtonName::West
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick => return,
        };
        match self.input.row {
            InputRow::Controller => self.change_controller(forward, effects),
            InputRow::Microphone => {
                let options = self.input.microphone_options();
                if let Some(next) = cycle(&options, &self.input.microphone, forward)
                    && next != self.input.microphone
                {
                    self.input.microphone = next.clone();
                    effects.push(Effect::SettingsMicrophone(next));
                }
            }
        }
    }

    /// Auto keeps every pad, so it applies at once; one named pad must confirm with A within 15 s.
    fn change_controller(&mut self, forward: bool, effects: &mut Vec<Effect>) {
        let options = self.input.controller_options();
        let Some(next) = cycle(&options, &self.input.controller, forward) else {
            return;
        };
        if next == self.input.controller {
            return;
        }
        let previous = core::mem::replace(&mut self.input.controller, next.clone());
        if next == ControllerChoice::Auto {
            effects.push(Effect::PersistController(next));
        } else {
            self.previous_controller = Some(previous);
            self.remaining = Some(Duration::from_secs(15));
            effects.push(Effect::SettingsController(next));
        }
    }

    fn display_button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        if self.selected == self.modes.len() && self.remaining.is_none() {
            let scale = if name == ButtonName::DpadLeft {
                self.scale.get().saturating_sub(1).max(1)
            } else if name == ButtonName::DpadRight {
                self.scale.get().saturating_add(1).min(i32::MAX.cast_unsigned())
            } else {
                self.scale.get()
            };
            if let Some(scale) = core::num::NonZeroU32::new(scale)
                && scale != self.scale
            {
                self.scale = scale;
                effects.push(Effect::SettingsScale(scale));
            }
            if matches!(
                name,
                ButtonName::DpadLeft | ButtonName::DpadRight | ButtonName::South
            ) {
                return;
            }
        }
        match name {
            ButtonName::East => self.back(effects),
            ButtonName::South => {
                if self.remaining.take().is_some() {
                    self.previous = None;
                    effects.push(Effect::PersistMode(self.current));
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
                self.selected = self.selected.saturating_add(1).min(self.modes.len());
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

fn is_open_phrase(text: &str) -> bool {
    text.trim()
        .trim_end_matches('.')
        .eq_ignore_ascii_case("pop the hood")
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open: false,
            page: Page::Categories,
            category: Category::Display,
            input: InputSettings::default(),
            selected: 0,
            current: 0,
            modes: Vec::new(),
            wallpapers: Vec::new(),
            wallpaper_locked: Vec::new(),
            wallpaper_selected: 0,
            wallpaper_current: 0,
            scale: core::num::NonZeroU32::MIN,
            remaining: None,
            previous: None,
            previous_controller: None,
            status: Status::Ready,
            cancel_preview: false,
            south: None,
            held: Vec::new(),
        }
    }
}
