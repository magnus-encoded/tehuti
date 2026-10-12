//! Protocol v0 types shared by the core and plugins.

/// A controller button. Face buttons are named by position: South is A on an Xbox pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
#[serde(rename_all = "snake_case")]
pub enum ButtonName {
    South,
    East,
    North,
    West,
    LeftBumper,
    RightBumper,
    Select,
    Start,
    Guide,
    LeftStick,
    RightStick,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum ButtonState {
    Pressed,
    Released,
}

/// Input that reaches the core.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum Event {
    Button { name: ButtonName, state: ButtonState },
    Focus { app_id: Option<String>, title: String },
    Tick { elapsed: core::time::Duration },
}

/// A key the core can send to the focused client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum KeyName {
    Enter,
    Escape,
    Space,
    Tab,
    Backspace,
    #[expect(clippy::min_ident_chars, reason = "Up matches the other direction key names")]
    Up,
    Down,
    Left,
    Right,
}

impl KeyName {
    #[must_use]
    pub const fn evdev_code(self) -> u32 {
        match self {
            Self::Enter => 28,
            Self::Escape => 1,
            Self::Space => 57,
            Self::Tab => 15,
            Self::Backspace => 14,
            Self::Up => 103,
            Self::Down => 108,
            Self::Left => 105,
            Self::Right => 106,
        }
    }
}

/// An exact application-id match and its pressed-button mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    app_id: String,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
}

impl Context {
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    #[must_use]
    pub fn key_for(&self, button: ButtonName) -> Option<KeyName> {
        self.buttons.get(&button).and_then(|action| match action {
            MappingAction::Key { key } => Some(*key),
            MappingAction::Text { .. } => None,
        })
    }

    #[must_use]
    pub fn action_for(&self, button: ButtonName) -> Option<Action> {
        self.buttons.get(&button).map(|action| match action {
            MappingAction::Key { key } => Action::Key { key: *key },
            MappingAction::Text { text } => Action::Text { text: text.clone() },
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextFile {
    app_id: String,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
enum MappingAction {
    Key { key: KeyName },
    Text { text: String },
}

/// Parses a context at the configuration boundary.
///
/// # Errors
/// The TOML, button names, key names or application id are invalid.
pub fn parse_context(source: &str) -> Result<Context, String> {
    let context: ContextFile = toml::from_str(source).map_err(|error| error.to_string())?;
    if context.app_id.trim().is_empty() {
        return Err("app_id must not be empty".into());
    }
    if context
        .buttons
        .values()
        .any(|action| matches!(action, MappingAction::Text { text } if text.contains('\0')))
    {
        return Err("text must not contain NUL: Wayland strings cannot carry it".into());
    }
    Ok(Context {
        app_id: context.app_id,
        buttons: context.buttons,
    })
}

/// What the core asks its edges to do.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum Action {
    /// Press and release `key` in the focused client.
    Key { key: KeyName },
    /// Commit Unicode text to the focused client.
    Text { text: String },
    /// Release the controller and stop the compositor.
    Quit,
}
