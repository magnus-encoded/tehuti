//! Protocol v0 types shared by the core and plugins.

/// Text that can cross a Wayland string boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Text(String);

impl Text {
    /// Validates text before it becomes an action.
    ///
    /// # Errors
    /// The string contains NUL.
    pub fn new(text: String) -> Result<Self, String> {
        if text.contains('\0') {
            return Err("text must not contain NUL".into());
        }
        Ok(Self(text))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Text> for String {
    fn from(text: Text) -> Self {
        text.0
    }
}

impl TryFrom<String> for Text {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::new(text)
    }
}

#[cfg(feature = "arbitrary")]
impl proptest::arbitrary::Arbitrary for Text {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with((): Self::Parameters) -> Self::Strategy {
        use proptest::strategy::Strategy;
        proptest::collection::vec(
            proptest::arbitrary::any::<char>().prop_filter("NUL-free", |character| *character != '\0'),
            0..100,
        )
        .prop_map(|characters| Self(characters.into_iter().collect()))
        .boxed()
    }
}

/// A controller button. Face buttons are named by position: South is A on an Xbox pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    Record,
    Start,
    Guide,
    LeftStick,
    RightStick,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
#[serde(rename_all = "snake_case")]
pub enum ButtonState {
    Pressed,
    Released,
}

/// Input that reaches the core.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum Event {
    Button { name: ButtonName, state: ButtonState },
    Focus { app_id: Option<String>, title: String },
    Tick { elapsed: core::time::Duration },
    Speech { text: Text },
}

/// A key the core can send to the focused client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    speech: std::collections::HashMap<String, MappingAction>,
}

impl Context {
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    #[must_use]
    pub fn speech_action(&self, phrase: &str) -> Option<Action> {
        self.speech.get(phrase).map(MappingAction::action)
    }

    #[must_use]
    pub fn key_for(&self, button: ButtonName) -> Option<KeyName> {
        self.buttons.get(&button).and_then(|action| match action {
            MappingAction::Key { key } => Some(*key),
            MappingAction::Text { .. } | MappingAction::Secret { .. } => None,
        })
    }

    #[must_use]
    pub fn action_for(&self, button: ButtonName) -> Option<Action> {
        self.buttons.get(&button).map(|action| match action {
            MappingAction::Key { key } => Action::Key { key: *key },
            MappingAction::Text { text } => Action::Text { text: text.clone() },
            MappingAction::Secret { secret } => Action::SecretRequest {
                reference: secret.clone(),
            },
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextFile {
    app_id: String,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
    #[serde(default)]
    speech: std::collections::HashMap<String, MappingAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
enum MappingAction {
    Key { key: KeyName },
    Text { text: Text },
    Secret { secret: SecretReference },
}

impl MappingAction {
    fn action(&self) -> Action {
        match self {
            Self::Key { key } => Action::Key { key: *key },
            Self::Text { text } => Action::Text { text: text.clone() },
            Self::Secret { secret } => Action::SecretRequest {
                reference: secret.clone(),
            },
        }
    }
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
    Ok(Context {
        app_id: context.app_id,
        buttons: context.buttons,
        speech: context.speech,
    })
}

/// What the core asks its edges to do.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum Action {
    /// Press and release `key` in the focused client.
    Key { key: KeyName },
    /// Send Unicode text to the focused client.
    Text { text: Text },
    /// Ask the compositor to confirm delivery of a named secret reference.
    SecretRequest { reference: SecretReference },
    /// Release the controller and stop the compositor.
    Quit,
    /// Display a notice without changing the focused client.
    Notice { text: Text },
}

mod plugin;
pub use plugin::*;
