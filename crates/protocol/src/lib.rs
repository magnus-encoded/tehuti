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
    #[serde(alias = "A")]
    South,
    #[serde(alias = "B")]
    East,
    #[serde(alias = "Y")]
    North,
    #[serde(alias = "X")]
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
    /// Ctrl-C, which interrupts a terminal's foreground program.
    Interrupt,
}

impl KeyName {
    /// Whether the key is sent with Control held.
    #[must_use]
    pub const fn with_control(self) -> bool {
        matches!(self, Self::Interrupt)
    }

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
            Self::Interrupt => 46,
        }
    }
}

/// How a context responds to an enabled text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnTextField {
    Open,
    Button,
    Off,
}

/// Optional overrides of the global keyboard policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyboardConfig {
    pub on_text_field: Option<OnTextField>,
    pub button: Option<ButtonName>,
    /// Marks the application as a terminal, for apps that never report a terminal field.
    pub terminal: Option<bool>,
    /// Whether a Guide tap into the in-app level opens the keyboard in a terminal.
    pub open_on_in_app_level: Option<bool>,
}

/// An exact application-id match and its pressed-button mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    app_id: String,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
    speech: std::collections::HashMap<String, MappingAction>,
    unmatched_speech: Option<UnmatchedSpeech>,
    keyboard: KeyboardConfig,
}

impl Context {
    #[must_use]
    pub const fn keyboard(&self) -> &KeyboardConfig {
        &self.keyboard
    }

    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    #[must_use]
    pub fn speech_action(&self, phrase: &str) -> Option<Action> {
        let wanted = normalize_phrase(phrase);
        if phrase.trim().is_empty() {
            return None;
        }
        self.speech.get(&wanted).map(MappingAction::action).or_else(|| {
            let UnmatchedSpeech::Type = self.unmatched_speech?;
            Text::new(format!("{phrase} "))
                .ok()
                .map(|text| Action::Text { text })
        })
    }

    #[must_use]
    pub fn key_for(&self, button: ButtonName) -> Option<KeyName> {
        self.buttons.get(&button).and_then(|action| match action {
            MappingAction::Key { key } => Some(*key),
            MappingAction::Text { .. }
            | MappingAction::Secret { .. }
            | MappingAction::Launch { .. }
            | MappingAction::Close { .. }
            | MappingAction::Notify { .. }
            | MappingAction::Layout { .. } => None,
        })
    }

    #[must_use]
    pub fn action_for(&self, button: ButtonName) -> Option<Action> {
        self.buttons.get(&button).map(MappingAction::action)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum UnmatchedSpeech {
    #[serde(rename = "type")]
    Type,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextFile {
    app_id: String,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
    #[serde(default)]
    speech: std::collections::HashMap<String, MappingAction>,
    #[serde(default)]
    unmatched_speech: Option<UnmatchedSpeech>,
    #[serde(default)]
    keyboard: KeyboardConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
enum MappingAction {
    Key { key: KeyName },
    Text { text: Text },
    Secret { secret: SecretReference },
    Launch { launch: Text },
    Close { close: CloseMapping },
    Notify { notify: Text },
    Layout { layout: LayoutAction },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum CloseMapping {
    #[serde(rename = "focused")]
    Focused,
}

impl MappingAction {
    fn action(&self) -> Action {
        match self {
            Self::Launch { launch } => Action::Launch {
                desktop_entry: launch.clone(),
            },
            Self::Close { .. } => Action::Close,
            Self::Notify { notify } => Action::Notice { text: notify.clone() },
            Self::Key { key } => Action::Key { key: *key },
            Self::Text { text } => Action::Text { text: text.clone() },
            Self::Secret { secret } => Action::SecretRequest {
                reference: secret.clone(),
            },
            Self::Layout { layout } => Action::Layout { action: *layout },
        }
    }
}

/// Parses a context at the configuration boundary.
///
/// # Errors
/// The TOML, button names, key names or application id are invalid, or a speech
/// phrase normalizes to nothing or to the same phrase as another mapping.
pub fn parse_context(source: &str) -> Result<Context, String> {
    let context: ContextFile = toml::from_str(source).map_err(|error| error.to_string())?;
    if matches!(
        context.keyboard.button,
        Some(ButtonName::Guide | ButtonName::Record)
    ) {
        return Err("keyboard button cannot reserve Guide or Record".into());
    }
    if context.app_id.trim().is_empty() {
        return Err("app_id must not be empty".into());
    }
    let mut speech = std::collections::HashMap::with_capacity(context.speech.len());
    for (mapped, action) in context.speech {
        let normalized = normalize_phrase(&mapped);
        if normalized.is_empty() {
            return Err(format!(
                "speech phrase {mapped:?} has no words after normalization"
            ));
        }
        if speech.insert(normalized.clone(), action).is_some() {
            return Err(format!(
                "speech phrase {mapped:?} duplicates another phrase that also normalizes to {normalized:?}"
            ));
        }
    }
    Ok(Context {
        app_id: context.app_id,
        buttons: context.buttons,
        speech,
        unmatched_speech: context.unmatched_speech,
        keyboard: context.keyboard,
    })
}

/// What the core asks its edges to do.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum Action {
    /// Launch an installed desktop entry.
    Launch { desktop_entry: Text },
    /// Close the focused client and focus the most recently mapped remaining window.
    Close,
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
    /// Change the tiling layout; reserved for the core like `Close`.
    Layout { action: LayoutAction },
}

/// A tiling action a context maps to a button or phrase, written in kebab case,
/// such as `layout = "focus-column-left"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum LayoutAction {
    FocusColumnLeft,
    FocusColumnRight,
    FocusWindowUp,
    FocusWindowDown,
    MoveColumnLeft,
    MoveColumnRight,
    /// Cycles the focused column through the preset widths.
    SwitchPresetColumnWidth,
    FocusWorkspaceDown,
    FocusWorkspaceUp,
    MoveColumnToWorkspaceDown,
    MoveColumnToWorkspaceUp,
    ToggleFullscreen,
    MaximizeColumn,
}

mod plugin;
pub use plugin::*;

/// Folds case, whitespace and punctuation between and around words, so Whisper's
/// "Front. Center." matches "front center".
///
/// Apostrophes inside words are kept, with the typographic forms (’ and ʼ) folded to '. Any other character that is
/// not alphanumeric or a combining mark, including Unicode punctuation, separates
/// words. Text is NFC-normalized first so decomposed accents match composed ones.
#[must_use]
pub fn normalize_phrase(phrase: &str) -> String {
    use unicode_normalization::{UnicodeNormalization as _, char::is_combining_mark};
    phrase
        .to_lowercase()
        .nfc()
        .collect::<String>()
        .replace(['\u{2019}', '\u{2bc}'], "'")
        .split(|character: char| {
            !character.is_alphanumeric() && !is_combining_mark(character) && character != '\''
        })
        .map(|word| word.trim_matches('\''))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
