//! Shared state-flag convention for built-in modes and plugin-registered toggles.
//!
//! Voice addresses every mode as a destination (`"<name> on"` / `"<name> off"`, ADR 0008).
//! Controller navigation steps through the Toggles settings page or active bar chips.

use tehuti_protocol::{Toggle, normalize_phrase};

/// Built-in system modes owned by Tehuti.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuiltinMode {
    StayAwake,
    MicrophoneMute,
    DoNotDisturb,
}

impl BuiltinMode {
    /// Every built-in mode, in the order the Toggles page lists them.
    pub const ALL: [Self; 3] = [Self::StayAwake, Self::MicrophoneMute, Self::DoNotDisturb];

    /// The canonical spoken unit name voice uses with `on` or `off`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::StayAwake => "stay awake",
            Self::MicrophoneMute => "microphone mute",
            Self::DoNotDisturb => "do not disturb",
        }
    }

    /// Resolves a normalized phrase to its built-in mode.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

/// Who owns a mode toggle: Tehuti itself or a ready plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    Builtin(BuiltinMode),
    Plugin(usize),
}

/// One toggle entry on the Toggles page and (while active) on the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeToggle {
    pub name: String,
    pub owner: Owner,
    pub active: bool,
}

impl ModeToggle {
    /// Human-readable unit label with the exact words voice matches.
    #[must_use]
    pub fn display_name(&self) -> String {
        let mut chars = self.name.chars();
        chars.next().map_or_else(String::new, |first| {
            let mut text = String::with_capacity(self.name.len());
            text.extend(first.to_uppercase());
            text.push_str(chars.as_str());
            text
        })
    }
}

/// Result of setting a toggle by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetOutcome {
    pub toggle: ModeToggle,
    pub changed: bool,
}

/// Built-in and plugin-registered mode toggles in stable display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toggles {
    items: Vec<ModeToggle>,
}

impl Default for Toggles {
    fn default() -> Self {
        Self::new()
    }
}

impl Toggles {
    /// Starts with the built-in modes, all inactive.
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: BuiltinMode::ALL
                .into_iter()
                .map(|mode| ModeToggle {
                    name: mode.name().into(),
                    owner: Owner::Builtin(mode),
                    active: false,
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn items(&self) -> &[ModeToggle] {
        &self.items
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&ModeToggle> {
        self.items.get(index)
    }

    #[must_use]
    pub fn builtin_active(&self, mode: BuiltinMode) -> bool {
        self.items
            .iter()
            .any(|item| item.owner == Owner::Builtin(mode) && item.active)
    }

    #[must_use]
    pub fn is_active(&self, name: &str) -> bool {
        let normalized = normalize_phrase(name);
        self.items
            .iter()
            .any(|item| item.name == normalized && item.active)
    }

    /// Every currently active toggle, in display order.
    #[must_use]
    pub fn active_items(&self) -> Vec<&ModeToggle> {
        self.items.iter().filter(|item| item.active).collect()
    }

    /// Parses `"<mode> on"` or `"<mode> off"` for any registered mode toggle.
    #[must_use]
    pub fn parse_voice_destination(&self, phrase: &str) -> Option<(String, bool)> {
        let normalized = normalize_phrase(phrase);
        let (target, active) = normalized
            .strip_suffix(" on")
            .map(|target| (target, true))
            .or_else(|| normalized.strip_suffix(" off").map(|target| (target, false)))?;
        self.items
            .iter()
            .find(|item| item.name == target)
            .map(|item| (item.name.clone(), active))
    }

    /// Sets the named toggle to `active`, returning its state and whether it changed.
    pub fn set(&mut self, name: &str, active: bool) -> Option<SetOutcome> {
        let normalized = normalize_phrase(name);
        let item = self.items.iter_mut().find(|item| item.name == normalized)?;
        let changed = item.active != active;
        item.active = active;
        Some(SetOutcome {
            toggle: item.clone(),
            changed,
        })
    }

    /// Flips the toggle at `index` on the Toggles page.
    pub fn flip_at(&mut self, index: usize) -> Option<ModeToggle> {
        let item = self.items.get_mut(index)?;
        item.active = !item.active;
        Some(item.clone())
    }

    /// Turns off the `active_index`-th active toggle (focused on the bar).
    pub fn turn_off_active(&mut self, active_index: usize) -> Option<ModeToggle> {
        let item = self
            .items
            .iter_mut()
            .filter(|item| item.active)
            .nth(active_index)?;
        item.active = false;
        Some(item.clone())
    }

    /// Replaces the plugin-owned toggles while keeping built-in modes first and preserving
    /// first-seen order so late arrivals join the end without moving focus.
    pub fn sync_plugins(&mut self, registrations: Vec<(usize, Toggle)>) {
        let mut builtins = Vec::with_capacity(BuiltinMode::ALL.len());
        let mut known_plugins = Vec::new();
        for item in core::mem::take(&mut self.items) {
            match item.owner {
                Owner::Builtin(_) => builtins.push(item),
                Owner::Plugin(_) => known_plugins.push(item),
            }
        }
        let mut seen: std::collections::BTreeSet<String> =
            builtins.iter().map(|item| item.name.clone()).collect();
        let mut current = Vec::new();
        for (plugin, spec) in registrations {
            if seen.insert(spec.name().to_owned()) {
                current.push(ModeToggle {
                    name: spec.name().to_owned(),
                    owner: Owner::Plugin(plugin),
                    active: spec.active(),
                });
            }
        }
        let settled = crate::tiles::settle(&known_plugins, current, |item| (item.owner, item.name.clone()));
        builtins.extend(settled);
        self.items = builtins;
    }

    /// Drops every toggle owned by `plugin`.
    pub fn remove_plugin(&mut self, plugin: usize) {
        self.items.retain(|item| item.owner != Owner::Plugin(plugin));
    }
}
