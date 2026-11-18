//! Controller-owned settings pages and timed confirmation of risky changes.

use super::{
    Effect,
    input_settings::{
        CONTROLLER_ROW, CONTROLLER_SETTINGS_ROW, ControllerChoice, InputSettings, MICROPHONE_ROW,
        TALK_OPTIONS,
    },
};
use crate::pad::{PadFamily, Role};
use crate::tiles::{Direction, Focus, clamp, first, settle, step};
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
    pub(super) pad_family: PadFamily,
    pub open: bool,
    pub page: Page,
    /// The highlighted category on the categories page.
    pub category: Category,
    pub input: InputSettings,
    /// The focused tile of the Display page.
    pub display_focus: Focus,
    /// The resolution tile whose refresh rates the second row shows.
    pub display_resolution: usize,
    pub current: usize,
    pub modes: Vec<String>,
    pub wallpapers: Vec<String>,
    pub wallpaper_locked: Vec<bool>,
    pub wallpaper_selected: usize,
    pub wallpaper_current: usize,
    pub bar_font: crate::bar::Font,
    pub scale: core::num::NonZeroU32,
    pub remaining: Option<Duration>,
    previous: Option<usize>,
    status: Status,
    cancel_preview: bool,
    confirm: Option<ButtonState>,
    held: Vec<ButtonName>,
}

impl Settings {
    #[must_use]
    pub fn button_hint(&self) -> String {
        let confirm = self.pad_family.label(self.pad_family.button(Role::Confirm));
        let back = self.pad_family.label(self.pad_family.button(Role::Back));
        if self.remaining.is_some() {
            format!("{confirm}: KEEP THIS MODE")
        } else {
            match self.page {
                Page::Categories => format!("D-PAD: MOVE   {confirm}: OPEN   {back}: CLOSE"),
                Page::Input if self.input.pending_disable.is_some() => {
                    format!("{confirm}: DISABLE THE PAD IN USE   {back}: KEEP IT")
                }
                Page::Display | Page::Input => format!("D-PAD: MOVE   {confirm}: APPLY   {back}: BACK"),
                Page::Wallpaper => format!("D-PAD: PREVIEW   {confirm}: SAVE   {back}: CANCEL"),
            }
        }
    }

    /// Sets the connected pads shown on the Input page; a new pad joins the end of the row.
    pub(super) fn configure_controllers(&mut self, controllers: Vec<super::ControllerInfo>) {
        let focused = self.input.focused_controller().cloned();
        self.input.controllers = settle(&self.input.controllers, controllers, |controller| {
            controller.key.clone()
        });
        if let Some(col) = focused.and_then(|tile| {
            self.input
                .controllers
                .iter()
                .position(|controller| controller.key == tile.key)
        }) {
            self.input.focus.col = col;
        }
        if let Some(pending) = &self.input.pending_disable
            && !self
                .input
                .controllers
                .iter()
                .any(|controller| controller.key == *pending)
        {
            self.input.pending_disable = None;
        }
        self.keep_input_focus();
    }

    /// Sets the capture sources shown on the Input page; a new source joins the end of the row.
    pub(super) fn configure_microphones(&mut self, microphones: Vec<super::Microphone>) {
        let focused = self.input.focused_microphone();
        self.input.microphones = settle(&self.input.microphones, microphones, |microphone| {
            microphone.name.clone()
        });
        if let Some(col) = focused.and_then(|tile| {
            self.input
                .microphone_options()
                .iter()
                .position(|option| *option == tile)
        }) {
            self.input.focus.col = col;
        }
        self.keep_input_focus();
    }

    fn keep_input_focus(&mut self) {
        let rows = self.input.rows();
        let focus = self.input.focus;
        self.input.focus = if rows.get(focus.row).copied().unwrap_or(0) == 0 {
            first(&rows)
        } else {
            clamp(&rows, focus)
        };
        if self.input.focus.row == CONTROLLER_ROW {
            self.input.controller_cursor = self.input.focus.col;
        }
        self.input.controller_cursor = self
            .input
            .controller_cursor
            .min(self.input.controllers.len().saturating_sub(1));
    }

    /// Restores saved choices.
    pub(super) fn restore_input(
        &mut self,
        controller: ControllerChoice,
        microphone: super::MicrophoneChoice,
        disabled_controllers: Vec<String>,
        disabled_microphones: Vec<String>,
    ) {
        self.input.controller = controller;
        self.input.microphone = microphone;
        self.input.disabled_controllers = disabled_controllers;
        self.input.disabled_microphones = disabled_microphones;
    }

    #[must_use]
    pub fn failed(&self) -> bool {
        self.status == Status::Failed
    }

    pub(super) fn configure(&mut self, modes: Vec<String>, current: usize) {
        if current < modes.len() {
            self.modes = modes;
            self.current = current;
            if self.page == Page::Display {
                let resolutions = self.resolutions().len();
                self.display_resolution = self.display_resolution.min(resolutions.saturating_sub(1));
                self.display_focus = clamp(&self.display_rows(), self.display_focus);
            } else {
                self.focus_current_mode();
            }
        }
    }

    /// Puts display focus on the resolution of the active mode.
    fn focus_current_mode(&mut self) {
        let current = self.modes.get(self.current).map(|mode| resolution_of(mode));
        self.display_resolution = self
            .resolutions()
            .iter()
            .position(|name| Some(name.as_str()) == current)
            .unwrap_or(0);
        self.display_focus = Focus {
            row: 0,
            col: self.display_resolution,
        };
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

    fn back(&mut self, effects: &mut Vec<Effect>) {
        self.revert(effects);
        if self.page == Page::Wallpaper && self.wallpaper_selected != self.wallpaper_current {
            self.wallpaper_selected = self.wallpaper_current;
            effects.push(Effect::WallpaperPreview(self.wallpaper_current));
        }
        self.page = Page::Categories;
    }

    fn end_preview(&mut self, effects: &mut Vec<Effect>) {
        if self.status == Status::Switching {
            self.cancel_preview = true;
        } else {
            self.revert(effects);
        }
    }

    pub(super) fn close(&mut self, effects: &mut Vec<Effect>) {
        self.end_preview(effects);
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
        if self.open
            && self.page == Page::Input
            && let Event::Speech { text } = event
            && self.voice_destination(text.as_str(), effects)
        {
            return true;
        }
        let Event::Button { name, state } = event else {
            return false;
        };
        self.button_event(*name, *state, effects)
    }

    fn button_event(&mut self, name: ButtonName, state: ButtonState, effects: &mut Vec<Effect>) -> bool {
        if matches!(name, ButtonName::Guide | ButtonName::Start | ButtonName::Record) {
            return false;
        }
        let (was_held, stale_confirm) = self.track_button(name, state);
        if !self.open {
            return was_held;
        }
        if state == ButtonState::Released || stale_confirm {
            return true;
        }
        if self.status == Status::Switching {
            if name == self.pad_family.button(Role::Back) {
                self.cancel_preview = true;
                self.open = false;
                self.page = Page::Categories;
            }
            return true;
        }
        self.button(name, effects);
        true
    }

    fn track_button(&mut self, name: ButtonName, state: ButtonState) -> (bool, bool) {
        let stale_confirm = if name == self.pad_family.button(Role::Confirm) {
            let previous = self.confirm.replace(state);
            previous == Some(ButtonState::Pressed) && state == ButtonState::Pressed
        } else {
            false
        };
        let was_held = self.held.contains(&name);
        if state == ButtonState::Released {
            self.held.retain(|held| *held != name);
        } else if self.open && !was_held {
            self.held.push(name);
        }
        (was_held, stale_confirm)
    }

    fn toggle(&mut self, effects: &mut Vec<Effect>) {
        if self.open {
            self.close(effects);
        } else {
            self.end_preview(effects);
            self.open = true;
            self.page = Page::Categories;
        }
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
            name if name == self.pad_family.button(Role::Back) => self.close(effects),
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
            name if name == self.pad_family.button(Role::Confirm) => match self.category {
                Category::Display => {
                    self.page = Page::Display;
                    self.focus_current_mode();
                }
                Category::Input => {
                    self.page = Page::Input;
                    self.input.focus = first(&self.input.rows());
                    self.input.controller_cursor = 0;
                    self.input.pending_disable = None;
                    effects.push(Effect::ListMicrophones);
                }
                Category::Wallpaper => {
                    self.page = Page::Wallpaper;
                    self.wallpaper_selected = self.wallpaper_current;
                }
            },
            ButtonName::South
            | ButtonName::East
            | ButtonName::North
            | ButtonName::West
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::LeftTrigger
            | ButtonName::RightTrigger
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick
            | ButtonName::Paddle1
            | ButtonName::Paddle2
            | ButtonName::Paddle3
            | ButtonName::Paddle4 => {}
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
            name if name == self.pad_family.button(Role::Back) => self.back(effects),
            name if name == self.pad_family.button(Role::Confirm) && !self.wallpapers.is_empty() => {
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
            | ButtonName::East
            | ButtonName::North
            | ButtonName::West
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::LeftTrigger
            | ButtonName::RightTrigger
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick
            | ButtonName::Paddle1
            | ButtonName::Paddle2
            | ButtonName::Paddle3
            | ButtonName::Paddle4
            | ButtonName::DpadDown
            | ButtonName::DpadRight => {}
        }
    }

    fn input_button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        if name == self.pad_family.button(Role::Back) {
            if self.input.pending_disable.take().is_none() {
                self.back(effects);
            }
        } else if let Some(direction) = Direction::from_button(name) {
            self.input.pending_disable = None;
            let before = clamp(&self.input.rows(), self.input.focus);
            let mut after = step(&self.input.rows(), self.input.focus, direction);
            if before.row == CONTROLLER_ROW && after.row == CONTROLLER_SETTINGS_ROW {
                after.col = 0;
            } else if after.row == CONTROLLER_ROW {
                if before.row == CONTROLLER_SETTINGS_ROW {
                    after.col = self.input.controller_cursor;
                }
                self.input.controller_cursor = after.col;
            }
            self.input.focus = after;
        } else if name == self.pad_family.button(Role::Confirm) {
            self.apply_input_tile(effects);
        }
    }

    fn apply_input_tile(&mut self, effects: &mut Vec<Effect>) {
        let focus = clamp(&self.input.rows(), self.input.focus);
        match focus.row {
            CONTROLLER_ROW => {
                if let Some(name) = self
                    .input
                    .controllers
                    .get(focus.col)
                    .map(|controller| controller.key.clone())
                {
                    self.toggle_controller(name, effects);
                }
            }
            CONTROLLER_SETTINGS_ROW => {
                let Some(name) = self.input.selected_controller().map(|pad| pad.key.clone()) else {
                    return;
                };
                match focus.col.checked_sub(1) {
                    None => self.toggle_controller(name, effects),
                    Some(index) if index == TALK_OPTIONS.len() => self.step_deadzone(name, effects),
                    Some(index) if index == TALK_OPTIONS.len().saturating_add(1) => {
                        effects.push(Effect::RecentreController(name));
                    }
                    Some(index) => {
                        if let Some(option) = TALK_OPTIONS.get(index).copied() {
                            self.choose_talk(option, effects);
                        }
                    }
                }
            }
            MICROPHONE_ROW => match self.input.microphone_options().get(focus.col).cloned() {
                Some(super::MicrophoneChoice::Named(name)) => self.toggle_microphone(name, effects),
                Some(choice) => self.choose_microphone(choice, effects),
                None => {}
            },
            _ => {}
        }
    }

    /// Disabling the pad in use asks for a second confirm, so input is never lost by a slip.
    fn toggle_controller(&mut self, name: String, effects: &mut Vec<Effect>) {
        let enabled = self.input.controller_enabled(&name);
        if enabled
            && self.input.controller_in_use(&name)
            && self.input.pending_disable.as_deref() != Some(name.as_str())
        {
            self.input.pending_disable = Some(name);
            return;
        }
        effects.push(self.input.set_controller_enabled(name, !enabled));
    }

    /// A named microphone: an off one turns on, an on one is chosen, the chosen one turns off.
    fn toggle_microphone(&mut self, name: String, effects: &mut Vec<Effect>) {
        let choice = super::MicrophoneChoice::Named(name.clone());
        if !self.input.microphone_enabled(&name) {
            effects.push(self.input.set_microphone_enabled(name, true));
        } else if self.input.microphone != choice {
            self.choose_microphone(choice, effects);
        } else {
            self.silence_microphone(name, effects);
        }
    }

    /// Turns a microphone off; an off microphone is never the single chosen one.
    fn silence_microphone(&mut self, name: String, effects: &mut Vec<Effect>) {
        let chosen = self.input.microphone == super::MicrophoneChoice::Named(name.clone());
        if self.input.microphone_enabled(&name) {
            effects.push(self.input.set_microphone_enabled(name, false));
        }
        if chosen {
            self.choose_microphone(super::MicrophoneChoice::All, effects);
        }
    }

    fn choose_microphone(&mut self, choice: super::MicrophoneChoice, effects: &mut Vec<Effect>) {
        if choice != self.input.microphone {
            self.input.microphone = choice.clone();
            effects.push(Effect::SettingsMicrophone(choice));
        }
    }

    /// Moves the pad's stick deadzone to the next step and saves it.
    fn step_deadzone(&mut self, name: String, effects: &mut Vec<Effect>) {
        let next = self.input.next_deadzone(&name);
        let _previous = self.input.controller_deadzones.insert(name.clone(), next);
        effects.push(Effect::PersistControllerDeadzone(name, next));
    }

    fn choose_talk(&mut self, option: Option<ButtonName>, effects: &mut Vec<Effect>) {
        let Some(name) = self.input.selected_controller().map(|pad| pad.key.clone()) else {
            return;
        };
        if self.input.talk_choice() == option {
            return;
        }
        if let Some(button) = option {
            let _previous = self.input.controller_bindings.insert(name.clone(), button);
        } else {
            let _previous = self.input.controller_bindings.remove(&name);
        }
        effects.push(Effect::PersistControllerTalk(name, option));
    }

    /// A controller or microphone name followed by "on" or "off", spoken on the Input page.
    /// Voice may disable the pad in use: it is the way back when the pad is the problem.
    fn voice_destination(&mut self, phrase: &str, effects: &mut Vec<Effect>) -> bool {
        let phrase = tehuti_protocol::normalize_phrase(phrase);
        let (target, enable) = if let Some(target) = phrase.strip_suffix(" on") {
            (target, true)
        } else if let Some(target) = phrase.strip_suffix(" off") {
            (target, false)
        } else {
            return false;
        };
        let pad =
            self.input
                .controllers
                .iter()
                .find(|controller| controller.spoken_name() == target)
                .or_else(|| {
                    let mut same =
                        self.input.controllers.iter().filter(|controller| {
                            tehuti_protocol::normalize_phrase(&controller.name) == target
                        });
                    same.next().filter(|_| same.next().is_none())
                })
                .map(|controller| controller.key.clone());
        if let Some(name) = pad {
            effects.push(self.input.set_controller_enabled(name, enable));
            return true;
        }
        let microphone = self
            .input
            .microphones
            .iter()
            .find(|microphone| microphone.spoken_name() == target)
            .map(|microphone| microphone.name.clone());
        let Some(name) = microphone else {
            return false;
        };
        if enable {
            if !self.input.microphone_enabled(&name) {
                effects.push(self.input.set_microphone_enabled(name.clone(), true));
            }
            self.choose_microphone(super::MicrophoneChoice::Named(name), effects);
        } else {
            self.silence_microphone(name, effects);
        }
        true
    }

    /// One tile per distinct resolution among the modes, in driver order.
    #[must_use]
    pub fn resolutions(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for mode in &self.modes {
            let name = resolution_of(mode);
            if !names.iter().any(|known| known == name) {
                names.push(name.to_owned());
            }
        }
        names
    }

    /// The modes, by index, that share the resolution tile `resolution`: the refresh-rate row.
    #[must_use]
    pub fn refresh_modes(&self, resolution: usize) -> Vec<usize> {
        let names = self.resolutions();
        let Some(name) = names.get(resolution) else {
            return Vec::new();
        };
        let mut shown: Vec<usize> = Vec::new();
        for (index, mode) in self.modes.iter().enumerate() {
            if resolution_of(mode) != name.as_str() {
                continue;
            }
            let rate = rate_of(mode);
            let twin = shown
                .iter()
                .position(|known| self.modes.get(*known).is_some_and(|other| rate_of(other) == rate));
            match twin {
                None => shown.push(index),
                Some(place) if index == self.current => {
                    if let Some(slot) = shown.get_mut(place) {
                        *slot = index;
                    }
                }
                Some(_) => {}
            }
        }
        shown
    }

    /// The scale tiles: one to four, and the saved scale when it is larger.
    #[must_use]
    pub fn scale_steps(&self) -> Vec<u32> {
        let mut steps: Vec<u32> = (1..=4).collect();
        if !steps.contains(&self.scale.get()) {
            steps.push(self.scale.get());
        }
        steps
    }

    /// The tiles in each Display row: resolutions, refresh rates of the focused resolution, scales, fonts.
    #[must_use]
    pub fn display_rows(&self) -> [usize; 4] {
        [
            self.resolutions().len(),
            self.refresh_modes(self.display_resolution).len(),
            self.scale_steps().len(),
            crate::bar::Font::ALL.len(),
        ]
    }

    fn move_display_focus(&mut self, direction: Direction) {
        let rows = self.display_rows();
        let before = clamp(&rows, self.display_focus);
        let mut after = step(&rows, before, direction);
        if before.row == 0 && after.row == 0 {
            self.display_resolution = after.col;
        } else if before.row == 0 && after.row == 1 {
            after.col = self
                .refresh_modes(self.display_resolution)
                .iter()
                .position(|index| *index == self.current)
                .unwrap_or(0);
        } else if before.row == 1 && after.row == 0 {
            after.col = self.display_resolution;
        }
        self.display_focus = after;
    }

    fn apply_display_tile(&mut self, effects: &mut Vec<Effect>) {
        if self.remaining.take().is_some() {
            self.previous = None;
            effects.push(Effect::PersistMode(self.current));
            return;
        }
        let focus = clamp(&self.display_rows(), self.display_focus);
        match focus.row {
            0 => {
                let modes = self.refresh_modes(focus.col);
                let mode = modes
                    .iter()
                    .copied()
                    .find(|index| *index == self.current)
                    .or_else(|| modes.first().copied());
                self.apply_mode(mode, effects);
            }
            1 => {
                let mode = self
                    .refresh_modes(self.display_resolution)
                    .get(focus.col)
                    .copied();
                self.apply_mode(mode, effects);
            }
            2 => self.apply_scale_tile(focus.col, effects),
            _ => {
                if let Some(font) = crate::bar::Font::ALL.get(focus.col).copied() {
                    effects.push(Effect::SettingsBarFont(font));
                }
            }
        }
    }

    fn apply_scale_tile(&mut self, col: usize, effects: &mut Vec<Effect>) {
        if let Some(scale) = self
            .scale_steps()
            .get(col)
            .and_then(|value| core::num::NonZeroU32::new(*value))
            .filter(|scale| *scale != self.scale)
        {
            self.scale = scale;
            effects.push(Effect::SettingsScale(scale));
        }
    }

    fn apply_mode(&mut self, mode: Option<usize>, effects: &mut Vec<Effect>) {
        if let Some(mode) = mode.filter(|mode| *mode != self.current) {
            self.cancel_preview = false;
            self.previous = Some(self.current);
            self.status = Status::Switching;
            effects.push(Effect::SettingsMode(mode));
        }
    }

    fn display_button(&mut self, name: ButtonName, effects: &mut Vec<Effect>) {
        if name == self.pad_family.button(Role::Back) {
            self.back(effects);
        } else if self.remaining.is_some() {
            if name == self.pad_family.button(Role::Confirm) {
                self.apply_display_tile(effects);
            }
        } else if let Some(direction) = Direction::from_button(name) {
            self.move_display_focus(direction);
        } else if name == self.pad_family.button(Role::Confirm) {
            self.apply_display_tile(effects);
        }
    }
}

/// The `WxH` part of a `WxH@R` mode.
fn resolution_of(mode: &str) -> &str {
    mode.split_once('@').map_or(mode, |(size, _)| size)
}

/// The refresh text of a `WIDTHxHEIGHT@RATE` mode name.
fn rate_of(mode: &str) -> &str {
    mode.split_once('@').map_or("", |(_, rate)| rate)
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
            pad_family: PadFamily::default(),
            page: Page::Categories,
            category: Category::Display,
            input: InputSettings::default(),
            display_focus: Focus::default(),
            display_resolution: 0,
            current: 0,
            modes: Vec::new(),
            wallpapers: Vec::new(),
            wallpaper_locked: Vec::new(),
            wallpaper_selected: 0,
            wallpaper_current: 0,
            bar_font: crate::bar::Font::default(),
            scale: core::num::NonZeroU32::MIN,
            remaining: None,
            previous: None,
            status: Status::Ready,
            cancel_preview: false,
            confirm: None,
            held: Vec::new(),
        }
    }
}
