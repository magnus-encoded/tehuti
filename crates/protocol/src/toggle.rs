//! Validated, pre-registered mode toggles.

use serde::{Deserialize, Serialize};

/// The most toggles one plugin may register at once.
pub const MAX_TOGGLES: usize = 32;

/// The most characters one toggle name may have after normalization.
pub const MAX_TOGGLE_NAME_CHARS: usize = 24;

/// Built-in mode names owned by Tehuti; plugins cannot shadow them (ADR 0008).
pub const RESERVED_TOGGLE_NAMES: [&str; 3] = ["stay awake", "microphone mute", "do not disturb"];

/// A mode toggle registered by a plugin: its spoken unit name and current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Registration")]
pub struct Toggle {
    name: String,
    active: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    name: String,
    #[serde(default)]
    active: bool,
}

impl TryFrom<Registration> for Toggle {
    type Error = String;

    fn try_from(registration: Registration) -> Result<Self, String> {
        let Registration { name, active } = registration;
        Self::new(&name, active)
    }
}

/// Validates a toggle name for a `toggle` notice: nonempty, at most [`MAX_TOGGLE_NAME_CHARS`]
/// characters after normalization, without controls or trailing state words.
///
/// # Errors
/// Empty, too long, contains control characters, or ends with an ambiguous `on`/`off` word.
pub fn validate_toggle_name(raw: &str) -> Result<String, String> {
    if raw.chars().any(char::is_control) {
        return Err("toggle name must not contain control characters".into());
    }
    let name = crate::normalize_phrase(raw);
    if name.is_empty() || name.chars().count() > MAX_TOGGLE_NAME_CHARS {
        return Err(format!(
            "toggle name must be nonempty and at most {MAX_TOGGLE_NAME_CHARS} characters"
        ));
    }
    if name == "on" || name == "off" || name.ends_with(" on") || name.ends_with(" off") {
        return Err("toggle name must not end with on or off".into());
    }
    Ok(name)
}

/// Validates a plugin's toggle list: at most [`MAX_TOGGLES`] unique registrations.
///
/// # Errors
/// Too many toggles or duplicate normalized names.
pub fn validate_toggles(toggles: &[Toggle]) -> Result<(), String> {
    let mut names = std::collections::BTreeSet::new();
    if toggles.len() > MAX_TOGGLES || toggles.iter().any(|toggle| !names.insert(toggle.name())) {
        return Err(format!(
            "toggle names must be unique, with at most {MAX_TOGGLES} registrations"
        ));
    }
    Ok(())
}

impl Toggle {
    /// Creates a validated plugin toggle.
    ///
    /// # Errors
    /// Invalid or reserved toggle name.
    pub fn new(raw: &str, active: bool) -> Result<Self, String> {
        let name = validate_toggle_name(raw)?;
        if RESERVED_TOGGLE_NAMES.contains(&name.as_str()) {
            return Err(format!("toggle name `{name}` is reserved for built-in modes"));
        }
        Ok(Self { name, active })
    }

    pub const fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn active(&self) -> bool {
        self.active
    }
}
