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

macro_rules! key_names {
    ($(($variant:ident, $name:literal, $code:literal, $keysym:literal, $alias:literal)),+ $(,)?) => {
        /// A key the core can send to the focused client.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[expect(clippy::min_ident_chars, reason = "Variants use keyboard labels")]
        pub enum KeyName {
            $(
                #[serde(rename = $name, alias = $alias)]
                $variant,
            )+
        }

        impl KeyName {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            #[must_use]
            pub const fn evdev_code(self) -> u32 {
                match self {
                    $(Self::$variant => $code),+
                }
            }

            #[must_use]
            pub const fn keysym_name(self) -> &'static str {
                match self {
                    $(Self::$variant => $keysym),+
                }
            }
        }
    };
}

key_names! {
    (Escape, "escape", 1, "Escape", "Escape"),
    (Enter, "enter", 28, "Return", "Enter"),
    (Space, "space", 57, "space", "Space"),
    (Tab, "tab", 15, "Tab", "Tab"),
    (Backspace, "backspace", 14, "BackSpace", "Backspace"),
    (Up, "up", 103, "Up", "Up"),
    (Down, "down", 108, "Down", "Down"),
    (Left, "left", 105, "Left", "Left"),
    (Right, "right", 106, "Right", "Right"),
    (A, "a", 30, "a", "A"),
    (B, "b", 48, "b", "B"),
    (C, "c", 46, "c", "C"),
    (D, "d", 32, "d", "D"),
    (E, "e", 18, "e", "E"),
    (F, "f", 33, "f", "F"),
    (G, "g", 34, "g", "G"),
    (H, "h", 35, "h", "H"),
    (I, "i", 23, "i", "I"),
    (J, "j", 36, "j", "J"),
    (K, "k", 37, "k", "K"),
    (L, "l", 38, "l", "L"),
    (M, "m", 50, "m", "M"),
    (N, "n", 49, "n", "N"),
    (O, "o", 24, "o", "O"),
    (P, "p", 25, "p", "P"),
    (Q, "q", 16, "q", "Q"),
    (R, "r", 19, "r", "R"),
    (S, "s", 31, "s", "S"),
    (T, "t", 20, "t", "T"),
    (U, "u", 22, "u", "U"),
    (V, "v", 47, "v", "V"),
    (W, "w", 17, "w", "W"),
    (X, "x", 45, "x", "X"),
    (Y, "y", 21, "y", "Y"),
    (Z, "z", 44, "z", "Z"),
    (Digit0, "0", 11, "0", "Digit0"),
    (Digit1, "1", 2, "1", "Digit1"),
    (Digit2, "2", 3, "2", "Digit2"),
    (Digit3, "3", 4, "3", "Digit3"),
    (Digit4, "4", 5, "4", "Digit4"),
    (Digit5, "5", 6, "5", "Digit5"),
    (Digit6, "6", 7, "6", "Digit6"),
    (Digit7, "7", 8, "7", "Digit7"),
    (Digit8, "8", 9, "8", "Digit8"),
    (Digit9, "9", 10, "9", "Digit9"),
    (Minus, "minus", 12, "minus", "Minus"),
    (Equal, "equal", 13, "equal", "Equal"),
    (LeftBracket, "left_bracket", 26, "bracketleft", "LeftBracket"),
    (RightBracket, "right_bracket", 27, "bracketright", "RightBracket"),
    (Semicolon, "semicolon", 39, "semicolon", "Semicolon"),
    (Apostrophe, "apostrophe", 40, "apostrophe", "Apostrophe"),
    (Grave, "grave", 41, "grave", "Grave"),
    (Comma, "comma", 51, "comma", "Comma"),
    (Period, "period", 52, "period", "Period"),
    (Slash, "slash", 53, "slash", "Slash"),
    (Backslash, "backslash", 43, "backslash", "Backslash"),
    (F1, "f1", 59, "F1", "F1"),
    (F2, "f2", 60, "F2", "F2"),
    (F3, "f3", 61, "F3", "F3"),
    (F4, "f4", 62, "F4", "F4"),
    (F5, "f5", 63, "F5", "F5"),
    (F6, "f6", 64, "F6", "F6"),
    (F7, "f7", 65, "F7", "F7"),
    (F8, "f8", 66, "F8", "F8"),
    (F9, "f9", 67, "F9", "F9"),
    (F10, "f10", 68, "F10", "F10"),
    (F11, "f11", 87, "F11", "F11"),
    (F12, "f12", 88, "F12", "F12"),
    (Home, "home", 102, "Home", "Home"),
    (End, "end", 107, "End", "End"),
    (PageUp, "page_up", 104, "Prior", "PageUp"),
    (PageDown, "page_down", 109, "Next", "PageDown"),
    (Insert, "insert", 110, "Insert", "Insert"),
    (Delete, "delete", 111, "Delete", "Delete"),
}

#[cfg(feature = "arbitrary")]
impl proptest::arbitrary::Arbitrary for KeyName {
    type Parameters = ();
    type Strategy = proptest::sample::Select<Self>;

    fn arbitrary_with((): Self::Parameters) -> Self::Strategy {
        proptest::sample::select(Self::ALL)
    }
}

/// A modifier held during a key action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Super,
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
    /// Legacy option, accepted without opening on level changes.
    pub open_on_app_level: Option<bool>,
}

/// The fraction of a window's colour retained while dimmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(into = "f32")]
pub struct DimStrength(u16);

impl DimStrength {
    #[must_use]
    #[expect(
        clippy::float_arithmetic,
        reason = "validated thousandths become a colour multiplier"
    )]
    pub fn multiplier(self) -> f32 {
        f32::from(self.0) / 1000.0
    }
}

impl From<DimStrength> for f32 {
    fn from(value: DimStrength) -> Self {
        value.multiplier()
    }
}

impl<'de> serde::Deserialize<'de> for DimStrength {
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::float_arithmetic,
        reason = "finite unit interval is rounded into bounded thousandths"
    )]
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f32::deserialize(deserializer)?;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(serde::de::Error::custom("dim strength must be between 0 and 1"));
        }
        Ok(Self((value * 1000.0).round() as u16))
    }
}

/// Optional overrides of the global level dimming policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DimConfig {
    pub enabled: Option<bool>,
    pub strength: Option<DimStrength>,
    pub fade_ms: Option<u16>,
}

/// An exact application-id match and its pressed-button mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    app_id: String,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
    speech: std::collections::HashMap<String, MappingAction>,
    unmatched_speech: Option<UnmatchedSpeech>,
    keyboard: KeyboardConfig,
    dim: DimConfig,
    push_to_talk: Vec<ButtonName>,
    pad: PadConfig,
}

/// Controller ownership options for an application.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PadConfig {
    #[serde(default)]
    passthrough: bool,
    #[serde(default)]
    push_to_talk: Option<Vec<ButtonName>>,
}

impl Context {
    /// The buttons this context's `[buttons]` table maps, in no particular order.
    pub fn mapped_buttons(&self) -> impl Iterator<Item = ButtonName> + '_ {
        self.buttons.keys().copied()
    }

    #[must_use]
    pub fn push_to_talk(&self, button: ButtonName) -> bool {
        self.push_to_talk.contains(&button)
    }

    #[must_use]
    pub const fn dim(&self) -> &DimConfig {
        &self.dim
    }

    #[must_use]
    pub fn passthrough_push_to_talk(&self) -> &[ButtonName] {
        self.pad.push_to_talk.as_deref().unwrap_or(&[ButtonName::Record])
    }

    #[must_use]
    pub const fn passthrough(&self) -> bool {
        self.pad.passthrough
    }

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
        self.mapped_speech_action(phrase)
            .or_else(|| self.unmatched_speech_action(phrase))
    }

    /// The action of a mapped phrase only, ignoring `unmatched_speech`.
    #[must_use]
    pub fn mapped_speech_action(&self, phrase: &str) -> Option<Action> {
        if phrase.trim().is_empty() {
            return None;
        }
        self.speech
            .get(&normalize_phrase(phrase))
            .map(MappingAction::action)
    }

    /// The action `unmatched_speech` gives a phrase no mapping matched.
    #[must_use]
    pub fn unmatched_speech_action(&self, phrase: &str) -> Option<Action> {
        if phrase.trim().is_empty() {
            return None;
        }
        let UnmatchedSpeech::Type = self.unmatched_speech?;
        Text::new(format!("{phrase} "))
            .ok()
            .map(|text| Action::Text { text })
    }

    #[must_use]
    pub fn key_for(&self, button: ButtonName) -> Option<KeyName> {
        self.buttons.get(&button).and_then(|action| match action {
            MappingAction::Key { key, mods, .. } => mods.is_empty().then_some(*key),
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

    /// The key a `hold = true` mapping keeps down while `button` is held.
    #[must_use]
    pub fn held_key_for(&self, button: ButtonName) -> Option<KeyName> {
        match self.buttons.get(&button)? {
            MappingAction::Key { key, hold: true, .. } => Some(*key),
            MappingAction::Key { hold: false, .. }
            | MappingAction::Text { .. }
            | MappingAction::Secret { .. }
            | MappingAction::Launch { .. }
            | MappingAction::Close { .. }
            | MappingAction::Notify { .. }
            | MappingAction::Layout { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum UnmatchedSpeech {
    #[serde(rename = "type")]
    Type,
}

fn default_push_to_talk() -> Vec<ButtonName> {
    vec![ButtonName::Record]
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
    #[serde(default)]
    dim: DimConfig,
    #[serde(default = "default_push_to_talk")]
    push_to_talk: Vec<ButtonName>,
    #[serde(default)]
    pad: PadConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
enum MappingAction {
    Key {
        key: KeyName,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mods: Vec<Modifier>,
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        hold: bool,
    },
    Text {
        text: Text,
    },
    Secret {
        secret: SecretReference,
    },
    Launch {
        launch: Text,
    },
    Close {
        close: CloseMapping,
    },
    Notify {
        notify: Text,
    },
    Layout {
        layout: LayoutAction,
    },
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
            Self::Key { key, mods, .. } => Action::Key {
                key: *key,
                mods: mods.clone(),
            },
            Self::Text { text } => Action::Text { text: text.clone() },
            Self::Secret { secret } => Action::SecretRequest {
                reference: secret.clone(),
            },
            Self::Layout { layout } => Action::Layout { action: *layout },
        }
    }
}

fn validate_push_to_talk(buttons: Option<&[ButtonName]>) -> Result<(), String> {
    let Some(buttons) = buttons else {
        return Ok(());
    };
    if buttons.is_empty() || buttons.contains(&ButtonName::Guide) || buttons.contains(&ButtonName::Start) {
        return Err("pad.push_to_talk needs buttons other than Guide or Start".into());
    }
    if matches!(buttons, [button] if !matches!(button, ButtonName::Record | ButtonName::Select)) {
        return Err("a single pad.push_to_talk button must be Record or Select".into());
    }
    let mut seen = std::collections::HashSet::new();
    for button in buttons {
        if !seen.insert(button) {
            return Err("pad.push_to_talk cannot repeat a button".into());
        }
    }
    Ok(())
}

/// A held key is one plain key on a button: speech has no release, and held modifiers would leak.
fn validate_holds(context: &ContextFile) -> Result<(), String> {
    for (button, action) in &context.buttons {
        if matches!(action, MappingAction::Key { hold: true, mods, .. } if !mods.is_empty()) {
            return Err(format!("button {button:?} cannot hold a key with mods"));
        }
    }
    for (phrase, action) in &context.speech {
        if matches!(action, MappingAction::Key { hold: true, .. }) {
            return Err(format!(
                "speech phrase {phrase:?} cannot hold a key; hold is for buttons"
            ));
        }
    }
    Ok(())
}

/// Parses a context at the configuration boundary.
///
/// # Errors
/// The TOML, button names, key names or application id are invalid, a speech
/// phrase normalizes to nothing or to the same phrase as another mapping, or a
/// held key has mods or belongs to a phrase.
pub fn parse_context(source: &str) -> Result<Context, String> {
    let context: ContextFile = toml::from_str(source).map_err(|error| error.to_string())?;
    validate_push_to_talk(context.pad.push_to_talk.as_deref())?;
    validate_holds(&context)?;
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
        dim: context.dim,
        push_to_talk: context.push_to_talk,
        pad: context.pad,
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
    Key {
        key: KeyName,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mods: Vec<Modifier>,
    },
    /// Press `key` in the focused client and keep it down until [`Action::KeyUp`]; reserved for the core.
    KeyDown { key: KeyName },
    /// Release a key [`Action::KeyDown`] pressed; reserved for the core.
    KeyUp { key: KeyName },
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
    /// Cycles the focused column through the preset widths in reverse.
    SwitchPresetColumnWidthBack,
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
