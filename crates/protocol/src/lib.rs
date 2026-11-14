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
    /// The left analogue trigger, pressed once its axis passes a threshold.
    LeftTrigger,
    /// The right analogue trigger, pressed once its axis passes a threshold.
    RightTrigger,
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
    /// Keeps the local draft between keyboard openings.
    pub keep_draft: Option<bool>,
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

/// Which speech phrases the mapping ticker lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TickerSpeech {
    All,
    /// Only phrases with an icon, given or built in.
    Icons,
    /// Only the context's own phrases, none inherited from the global context.
    Own,
    None,
}

/// Optional overrides of the global mapping ticker policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TickerConfig {
    pub enabled: Option<bool>,
    pub speech: Option<TickerSpeech>,
    /// What keys do in this application, as the file spells them (`enter`, `ctrl+s`).
    #[serde(default)]
    pub names: std::collections::BTreeMap<String, String>,
    /// Display-only labels per pad button, as the file spells the button.
    #[serde(default)]
    pub buttons: std::collections::BTreeMap<String, String>,
}

/// Longest name a `[ticker.names]` entry may show, in characters.
pub const TICKER_NAME_MAX: usize = 24;

/// A key chord with a name the ticker shows instead of its keycap.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TickerName {
    key: KeyName,
    mods: Vec<Modifier>,
    name: String,
}

const fn modifier_rank(modifier: Modifier) -> u8 {
    match modifier {
        Modifier::Ctrl => 0,
        Modifier::Alt => 1,
        Modifier::Shift => 2,
        Modifier::Super => 3,
    }
}

fn sorted_mods(mods: &[Modifier]) -> Vec<Modifier> {
    let mut mods = mods.to_vec();
    mods.sort_by_key(|modifier| modifier_rank(*modifier));
    mods.dedup();
    mods
}

/// The labels `[ticker.buttons]` gives pad buttons, and the spellings that name no button.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TickerButtons {
    labels: Vec<(ButtonName, String)>,
    unknown: Vec<String>,
}

fn parse_ticker_buttons(
    labels: &std::collections::BTreeMap<String, String>,
) -> Result<TickerButtons, String> {
    let mut parsed: Vec<(ButtonName, String)> = Vec::with_capacity(labels.len());
    let mut unknown = Vec::new();
    for (spelled, label) in labels {
        let button = <ButtonName as serde::Deserialize>::deserialize(serde::de::value::StrDeserializer::<
            serde::de::value::Error,
        >::new(spelled))
        .ok()
        .filter(|button| *button != ButtonName::Guide);
        let Some(button) = button else {
            unknown.push(spelled.clone());
            continue;
        };
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Err(format!("ticker.buttons {spelled:?} must not be empty"));
        }
        if trimmed != label {
            return Err(format!(
                "ticker.buttons {spelled:?} must not have leading or trailing spaces"
            ));
        }
        if trimmed.chars().count() > TICKER_NAME_MAX {
            return Err(format!(
                "ticker.buttons {spelled:?} is longer than {TICKER_NAME_MAX} characters"
            ));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(format!(
                "ticker.buttons {spelled:?} must not contain control characters"
            ));
        }
        if parsed.iter().any(|(other, _)| *other == button) {
            return Err(format!("ticker.buttons {spelled:?} duplicates another entry"));
        }
        parsed.push((button, trimmed.to_owned()));
    }
    Ok(TickerButtons {
        labels: parsed,
        unknown,
    })
}

fn parse_ticker_names(names: &std::collections::BTreeMap<String, String>) -> Result<Vec<TickerName>, String> {
    fn word<'de, T: serde::Deserialize<'de>>(word: &'de str) -> Option<T> {
        T::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(
            word,
        ))
        .ok()
    }
    let mut parsed: Vec<TickerName> = Vec::with_capacity(names.len());
    for (spelled, name) in names {
        let mut parts: Vec<&str> = spelled.split('+').map(str::trim).collect();
        let key_word = parts.pop().unwrap_or_default();
        let key: KeyName = word(key_word).ok_or_else(|| {
            format!("ticker.names key {spelled:?} is not a key name (use names like enter, escape or ctrl+s)")
        })?;
        let mut mods = Vec::with_capacity(parts.len());
        for part in parts {
            mods.push(
                word::<Modifier>(part)
                    .ok_or_else(|| format!("ticker.names key {spelled:?} has unknown modifier {part:?}"))?,
            );
        }
        let mods = sorted_mods(&mods);
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(format!("ticker.names {spelled:?} must not be empty"));
        }
        if trimmed != name {
            return Err(format!(
                "ticker.names {spelled:?} must not have leading or trailing spaces"
            ));
        }
        if trimmed.chars().count() > TICKER_NAME_MAX {
            return Err(format!(
                "ticker.names {spelled:?} is longer than {TICKER_NAME_MAX} characters"
            ));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(format!(
                "ticker.names {spelled:?} must not contain control characters"
            ));
        }
        if parsed.iter().any(|other| other.key == key && other.mods == mods) {
            return Err(format!("ticker.names {spelled:?} duplicates another entry"));
        }
        parsed.push(TickerName {
            key,
            mods,
            name: trimmed.to_owned(),
        });
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherSettings {
    pub auto_pick: Option<bool>,
    pub delay_ms: Option<core::num::NonZeroU32>,
    pub jev: Option<bool>,
}

/// A mapped phrase as the context file spells it, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechMapping<'context> {
    pub phrase: &'context str,
    pub action: Action,
    pub icon: Option<&'context str>,
}

/// An exact application-id match and its pressed-button mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    app_id: String,
    title: Option<String>,
    label: Option<String>,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
    sticks: Vec<StickMapping>,
    speech: std::collections::HashMap<String, SpeechEntry>,
    /// Words that prime speech recognition while this context is focused.
    hotwords: Vec<String>,
    /// Each non-template phrase's [`speech_variant_key`] to its `speech` key, or
    /// `None` once two distinct phrases share the folded form.
    speech_variants: std::collections::HashMap<String, Option<String>>,
    /// Problems that did not stop the load, for the edge to log.
    warnings: Vec<String>,
    skipped_phrases: Vec<String>,
    unmatched_speech: Option<UnmatchedSpeech>,
    launch_phrases: Vec<(String, String)>,
    favourites: Vec<String>,
    keyboard: KeyboardConfig,
    dim: DimConfig,
    push_to_talk: Vec<ButtonName>,
    window: WindowConfig,
    log: LogConfig,
    pad: PadConfig,
    pointer: PointerConfig,
    ticker: TickerConfig,
    ticker_names: Vec<TickerName>,
    ticker_buttons: TickerButtons,
    speech_matching: SpeechMatching,
    launcher: LauncherSettings,
    projects: Vec<(String, String)>,
}

/// A speech mapping with its spelling in the file, its fuzzy key and an optional ticker icon.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpeechEntry {
    phrase: String,
    folded: String,
    action: MappingAction,
    icon: Option<Text>,
}

impl SpeechEntry {
    fn is_template(&self) -> bool {
        self.phrase.ends_with("<name>") || self.phrase.ends_with("<project>")
    }
}

#[derive(serde::Deserialize)]
struct SpeechEntryFile {
    #[serde(default)]
    icon: Option<Text>,
    #[serde(flatten)]
    action: MappingAction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PointerConfig {
    physical: bool,
}

/// Controller ownership options for an application.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PadConfig {
    #[serde(default)]
    passthrough: bool,
    #[serde(default, rename = "virtual")]
    virtual_pad: bool,
    #[serde(default)]
    push_to_talk: Option<Vec<ButtonName>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WindowConfig {
    open_fullscreen: bool,
    column_width: Option<WindowFraction>,
    height: Option<WindowFraction>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WindowFraction(f64);

impl Eq for WindowFraction {}

impl<'de> serde::Deserialize<'de> for WindowFraction {
    #[expect(
        clippy::float_arithmetic,
        reason = "named presets resolve to working-area fractions"
    )]
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Setting {
            Fraction(f64),
            Preset(String),
        }
        let value = match Setting::deserialize(deserializer)? {
            Setting::Fraction(value) => value,
            Setting::Preset(name) => match name.as_str() {
                "one-third" => 1.0 / 3.0,
                "one-half" => 0.5,
                "two-thirds" => 2.0 / 3.0,
                "full" => 1.0,
                _ => return Err(serde::de::Error::custom("unknown window size preset")),
            },
        };
        if !value.is_finite() || value <= 0.0 || value > 1.0 {
            return Err(serde::de::Error::custom(
                "window size must be greater than 0 and at most 1",
            ));
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LogConfig {
    window_size: bool,
}

impl Context {
    #[must_use]
    pub const fn physical_pointer(&self) -> bool {
        self.pointer.physical && !self.pad.passthrough
    }

    #[must_use]
    pub const fn log_window_size(&self) -> bool {
        self.log.window_size
    }

    #[must_use]
    pub const fn open_fullscreen(&self) -> bool {
        self.window.open_fullscreen
    }

    /// Opening column width as a fraction of the working area.
    #[must_use]
    pub fn window_column_width(&self) -> Option<f64> {
        self.window.column_width.map(|value| value.0)
    }

    /// Opening window height as a fraction of the working area.
    #[must_use]
    pub fn window_height(&self) -> Option<f64> {
        self.window.height.map(|value| value.0)
    }

    /// Ordered desktop entry ids eligible for desktop grid focus.
    #[must_use]
    pub fn favourites(&self) -> &[String] {
        &self.favourites
    }

    /// First configured launch phrase for each desktop entry, in file order.
    #[must_use]
    pub fn launch_phrases(&self) -> &[(String, String)] {
        &self.launch_phrases
    }

    /// The buttons this context's `[buttons]` table maps, in no particular order.
    pub fn mapped_buttons(&self) -> impl Iterator<Item = ButtonName> + '_ {
        self.buttons.keys().copied()
    }

    /// Every `[sticks]` mapping, in no particular order.
    #[must_use]
    pub fn stick_mappings(&self) -> &[StickMapping] {
        &self.sticks
    }

    /// Whether `[sticks]` maps any direction of `stick`, which then leaves mouse mode.
    #[must_use]
    pub fn maps_stick(&self, stick: Stick) -> bool {
        self.sticks.iter().any(|mapping| mapping.stick == stick)
    }

    /// Whether a `[sticks]` entry names `button` in `with`.
    #[must_use]
    pub fn stick_modifier(&self, button: ButtonName) -> bool {
        self.sticks.iter().any(|mapping| mapping.with == Some(button))
    }

    /// The mapping for `stick` pointing at `direction` with `with` held.
    #[must_use]
    pub fn stick_mapping(
        &self,
        stick: Stick,
        direction: StickDirection,
        with: Option<ButtonName>,
    ) -> Option<&StickMapping> {
        self.sticks
            .iter()
            .find(|mapping| mapping.stick == stick && mapping.direction == direction && mapping.with == with)
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
    pub const fn ticker(&self) -> &TickerConfig {
        &self.ticker
    }

    /// The label `[ticker.buttons]` gives a pad button, if any.
    #[must_use]
    pub fn ticker_button(&self, button: ButtonName) -> Option<&str> {
        self.ticker_buttons
            .labels
            .iter()
            .find(|(labelled, _)| *labelled == button)
            .map(|(_, label)| label.as_str())
    }

    /// `[ticker.buttons]` keys that name no pad button; they show nothing.
    #[must_use]
    pub fn unknown_ticker_buttons(&self) -> &[String] {
        &self.ticker_buttons.unknown
    }

    /// The name `[ticker.names]` gives a key chord, if any; modifier order does not matter.
    #[must_use]
    pub fn ticker_name(&self, key: KeyName, mods: &[Modifier]) -> Option<&str> {
        let mods = sorted_mods(mods);
        self.ticker_names
            .iter()
            .find(|entry| entry.key == key && entry.mods == mods)
            .map(|entry| entry.name.as_str())
    }

    #[must_use]
    pub const fn launcher(&self) -> &LauncherSettings {
        &self.launcher
    }

    #[must_use]
    pub const fn speech_matching(&self) -> &SpeechMatching {
        &self.speech_matching
    }

    /// Every mapped phrase with its normalized form, for fuzzy matching.
    pub fn speech_candidates(&self) -> impl Iterator<Item = (&str, Candidate<'_>)> {
        self.speech
            .iter()
            .filter(|(_, entry)| !entry.is_template())
            .map(|(normalized, entry)| {
                (
                    normalized.as_str(),
                    Candidate {
                        phrase: &entry.phrase,
                        folded: &entry.folded,
                        action: entry.action.action(),
                    },
                )
            })
    }

    /// The buttons that start speech capture, from a global context.
    #[must_use]
    pub fn push_to_talk_buttons(&self) -> &[ButtonName] {
        &self.push_to_talk
    }

    /// Every mapped phrase, in no particular order.
    pub fn speech_mappings(&self) -> impl Iterator<Item = SpeechMapping<'_>> {
        self.speech.values().map(|entry| SpeechMapping {
            phrase: &entry.phrase,
            action: entry.action.action(),
            icon: entry.icon.as_ref().map(Text::as_str),
        })
    }

    #[must_use]
    pub fn passthrough_push_to_talk(&self) -> &[ButtonName] {
        self.pad.push_to_talk.as_deref().unwrap_or(&[ButtonName::Record])
    }

    #[must_use]
    pub const fn passthrough(&self) -> bool {
        self.pad.passthrough
    }

    /// Whether passthrough presents a virtual controller that applies plugin overrides.
    #[must_use]
    pub const fn virtual_pad(&self) -> bool {
        self.pad.passthrough && self.pad.virtual_pad
    }

    #[must_use]
    pub const fn keyboard(&self) -> &KeyboardConfig {
        &self.keyboard
    }

    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// The exact window title this context also requires, if any.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The `[speech] hotwords` that prime recognition while this context is focused.
    #[must_use]
    pub fn hotwords(&self) -> &[String] {
        &self.hotwords
    }

    /// The short name the voice bar shows for this application's windows.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    #[must_use]
    pub fn speech_action(&self, phrase: &str) -> Option<Action> {
        self.mapped_speech_action(phrase)
            .or_else(|| self.folded_speech_action(phrase))
            .or_else(|| self.plural_speech_action(phrase))
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
            .filter(|entry| !entry.is_template())
            .map(|entry| entry.action.action())
            .or_else(|| {
                let mut matches = self.speech.values().filter_map(|entry| {
                    speech_templates::expand(&entry.phrase, &entry.action, phrase, &self.projects)
                });
                let first = matches.next()?;
                matches.all(|action| action == first).then_some(first)
            })
    }

    /// Problems found while loading that left the context usable, such as two
    /// phrases whose folded form was skipped because they share it.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Speech phrases dropped while loading, each message naming the phrase.
    #[must_use]
    pub fn skipped_phrases(&self) -> &[String] {
        &self.skipped_phrases
    }

    /// The action of a phrase the transcript matches once digits and hyphens
    /// fold; callers try it only once no exact phrase matched anywhere.
    #[must_use]
    pub fn folded_speech_action(&self, phrase: &str) -> Option<Action> {
        self.variant_lookup(&speech_variant_key(phrase))
            .map(|entry| entry.action.action())
    }

    fn variant_lookup(&self, variant: &str) -> Option<&SpeechEntry> {
        self.speech_variants
            .get(variant)?
            .as_ref()
            .and_then(|normalized| self.speech.get(normalized))
    }

    /// The action of a phrase whose last word is the plain plural or singular
    /// of the transcript's, when the shorter form has at least four letters;
    /// callers try it only once no exact or folded phrase matched anywhere.
    #[must_use]
    pub fn plural_speech_action(&self, phrase: &str) -> Option<Action> {
        const SHORTEST: usize = 4;
        let long_enough =
            |word: &str| word.chars().filter(|character| character.is_alphabetic()).count() >= SHORTEST;
        let mut words = speech_variant_words(phrase);
        let last = words.pop()?;
        let before = words.concat();
        last.strip_suffix('s')
            .filter(|singular| long_enough(singular))
            .and_then(|singular| self.variant_lookup(&format!("{before}{singular}")))
            .or_else(|| {
                long_enough(&last)
                    .then(|| self.variant_lookup(&format!("{before}{last}s")))
                    .flatten()
            })
            .map(|entry| entry.action.action())
    }

    /// Supplies discovered project names and absolute paths for `<project>` templates.
    pub fn set_projects(&mut self, projects: Vec<(String, String)>) {
        self.projects = projects;
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
            | MappingAction::Restart { .. }
            | MappingAction::Notify { .. }
            | MappingAction::Layout { .. }
            | MappingAction::Ticker { .. } => None,
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
            | MappingAction::Restart { .. }
            | MappingAction::Notify { .. }
            | MappingAction::Layout { .. }
            | MappingAction::Ticker { .. } => None,
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextFile {
    app_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    label: Option<Text>,
    #[serde(default)]
    favourites: Vec<String>,
    buttons: std::collections::HashMap<ButtonName, MappingAction>,
    #[serde(default)]
    sticks: sticks::SticksFile,
    #[serde(default)]
    speech: toml::Table,
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
    #[serde(default)]
    pointer: PointerConfig,
    #[serde(default)]
    window: WindowConfig,
    #[serde(default)]
    log: LogConfig,
    #[serde(default)]
    ticker: TickerConfig,
    #[serde(default)]
    speech_matching: SpeechMatching,
    #[serde(default)]
    launcher: LauncherSettings,
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
    Restart {
        restart: RestartMapping,
    },
    Notify {
        notify: Text,
    },
    Layout {
        layout: LayoutAction,
    },
    Ticker {
        ticker: TickerMapping,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestartMapping;

impl serde::Serialize for RestartMapping {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> serde::Deserialize<'de> for RestartMapping {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("restart must be true"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum CloseMapping {
    #[serde(rename = "focused")]
    Focused,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum TickerMapping {
    #[serde(rename = "toggle")]
    Toggle,
}

impl MappingAction {
    fn action(&self) -> Action {
        match self {
            Self::Launch { launch } => Action::Launch {
                desktop_entry: launch.clone(),
            },
            Self::Close { .. } => Action::Close,
            Self::Restart { .. } => Action::Restart,
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
            Self::Ticker { .. } => Action::ToggleTicker,
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
    Ok(())
}

fn parse_label(label: Option<Text>) -> Result<Option<String>, String> {
    let label = label.map(|label| label.as_str().trim().to_owned());
    if label.as_ref().is_some_and(String::is_empty) {
        return Err("label must not be empty".into());
    }
    Ok(label)
}

/// Parses a context at the configuration boundary.
///
/// # Errors
/// The TOML, button names, key names or application id are invalid, a speech
/// phrase or a held key has mods or belongs to a phrase. A phrase that
/// normalizes to nothing or to the same text as an earlier phrase is dropped
/// with a warning instead; the rest of the context loads.
pub fn parse_context(source: &str) -> Result<Context, String> {
    let context: ContextFile = toml::from_str(source).map_err(|error| error.to_string())?;
    validate_push_to_talk(context.pad.push_to_talk.as_deref())?;
    validate_holds(&context)?;
    validate_keyboard_button(context.keyboard.button)?;
    validate_app_id(&context.app_id)?;
    validate_title(&context.app_id, context.title.as_deref())?;
    let mut context = context;
    let hotwords = take_hotwords(&mut context.speech)?;
    let mut speech: std::collections::HashMap<String, SpeechEntry> =
        std::collections::HashMap::with_capacity(context.speech.len());
    let mut speech_variants = std::collections::HashMap::with_capacity(context.speech.len());
    let mut warnings = Vec::new();
    let mut skipped_phrases = Vec::new();
    let mut launch_phrases = Vec::new();
    for (mapped, value) in context.speech {
        let entry = speech_entry(&mapped, value, &mut launch_phrases)?;
        let normalized = match accept_phrase(&speech, &mapped) {
            Ok(normalized) => normalized,
            Err(message) => {
                skipped_phrases.push(message);
                continue;
            }
        };
        let template = entry.is_template();
        drop(speech.insert(normalized.clone(), entry));
        warnings.extend(insert_speech_variant(
            &mut speech_variants,
            &mapped,
            normalized,
            template,
        ));
    }
    Ok(Context {
        app_id: context.app_id,
        title: context.title,
        label: parse_label(context.label)?,
        buttons: context.buttons,
        sticks: sticks::parse_sticks(context.sticks)?,
        speech,
        hotwords,
        speech_variants,
        warnings,
        skipped_phrases,
        launch_phrases,
        favourites: context.favourites,
        unmatched_speech: context.unmatched_speech,
        keyboard: context.keyboard,
        dim: context.dim,
        push_to_talk: context.push_to_talk,
        pad: context.pad,
        pointer: context.pointer,
        window: context.window,
        log: context.log,
        ticker_names: parse_ticker_names(&context.ticker.names)?,
        ticker_buttons: parse_ticker_buttons(&context.ticker.buttons)?,
        ticker: context.ticker,
        speech_matching: context.speech_matching,
        launcher: context.launcher,
        projects: Vec::new(),
    })
}

/// The normalized phrase, or why the phrase is dropped: it has no words or
/// clashes with an earlier phrase.
fn accept_phrase(
    speech: &std::collections::HashMap<String, SpeechEntry>,
    mapped: &str,
) -> Result<String, String> {
    let normalized =
        normalized_speech_phrase(mapped).map_err(|message| format!("speech phrase skipped: {message}"))?;
    match speech.get(&normalized) {
        Some(earlier) => Err(format!(
            "speech phrase {mapped:?} skipped: clashes with earlier phrase {:?}, both normalize to {normalized:?}",
            earlier.phrase
        )),
        None => Ok(normalized),
    }
}

fn validate_keyboard_button(button: Option<ButtonName>) -> Result<(), String> {
    if matches!(button, Some(ButtonName::Guide | ButtonName::Record)) {
        return Err("keyboard button cannot reserve Guide or Record".into());
    }
    Ok(())
}

/// Removes the `hotwords` array from the `[speech]` table; a `hotwords` phrase
/// mapped to a table stays a phrase.
fn take_hotwords(speech: &mut toml::Table) -> Result<Vec<String>, String> {
    if !speech.get("hotwords").is_some_and(toml::Value::is_array) {
        return Ok(Vec::new());
    }
    let Some(toml::Value::Array(values)) = speech.remove("hotwords") else {
        return Ok(Vec::new());
    };
    values
        .into_iter()
        .map(|value| match value {
            toml::Value::String(word) => Ok(word),
            _ => Err("speech hotwords must be strings".to_owned()),
        })
        .collect()
}

fn speech_entry(
    mapped: &str,
    value: toml::Value,
    launch_phrases: &mut Vec<(String, String)>,
) -> Result<SpeechEntry, String> {
    let entry: SpeechEntryFile = value
        .try_into()
        .map_err(|error: toml::de::Error| error.to_string())?;
    if matches!(entry.action, MappingAction::Key { hold: true, .. }) {
        return Err(format!(
            "speech phrase {mapped:?} cannot hold a key; hold is for buttons"
        ));
    }
    record_launch_phrase(launch_phrases, &entry.action, mapped);
    speech_templates::validate(mapped, &entry.action)?;
    Ok(SpeechEntry {
        folded: fold_phrase(mapped),
        phrase: mapped.to_owned(),
        action: entry.action,
        icon: entry.icon,
    })
}

fn record_launch_phrase(phrases: &mut Vec<(String, String)>, action: &MappingAction, mapped: &str) {
    if let MappingAction::Launch { launch } = action
        && !phrases
            .iter()
            .any(|(desktop_entry, _)| desktop_entry == launch.as_str())
    {
        phrases.push((launch.as_str().to_owned(), mapped.to_owned()));
    }
}

/// Records a phrase's folded form; a second distinct phrase with the same form
/// skips it for both and returns a warning naming them.
fn insert_speech_variant(
    variants: &mut std::collections::HashMap<String, Option<String>>,
    mapped: &str,
    normalized: String,
    template: bool,
) -> Option<String> {
    if template {
        return None;
    }
    let variant = speech_variant_key(mapped);
    let Some(slot) = variants.get_mut(&variant) else {
        variants.extend([(variant, Some(normalized))]);
        return None;
    };
    let other = slot.take();
    Some(format!(
        "speech phrases {mapped:?} and {} fold alike to {variant:?}; only their exact forms match",
        other.map_or_else(|| "others".to_owned(), |other| format!("{other:?}")),
    ))
}

fn normalized_speech_phrase(mapped: &str) -> Result<String, String> {
    let normalized = normalize_phrase(mapped);
    if normalized.is_empty() {
        return Err(format!(
            "speech phrase {mapped:?} has no words after normalization"
        ));
    }
    Ok(normalized)
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
    /// Release the controller and restart the compositor in its session.
    Restart,
    /// Display a notice without changing the focused client.
    Notice { text: Text },
    /// Change the tiling layout; reserved for the core like `Close`.
    Layout { action: LayoutAction },
    /// Show or hide the mapping ticker; reserved for the core.
    ToggleTicker,
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

mod fuzzy;
mod plugin;
pub use fuzzy::*;
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

/// The looser form a phrase and a transcript are compared in after the exact lookup fails.
///
/// It is [`normalize_phrase`], then each standalone number 0 to 20 as its
/// word (not with a leading zero; a digit run glued to letters counts as its own word), then the word breaks removed, so "camera-1", "camera 1" and "Camera one"
/// agree, as do "cock-pit" and "cockpit".
#[must_use]
fn speech_variant_key(phrase: &str) -> String {
    speech_variant_words(phrase).concat()
}

/// The folded words of a phrase, before [`speech_variant_key`] joins them.
fn speech_variant_words(phrase: &str) -> Vec<String> {
    const NUMBERS: [&str; 21] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    let mut spaced = String::new();
    let mut previous: Option<char> = None;
    for character in normalize_phrase(phrase).chars() {
        if previous.is_some_and(|before| {
            before != ' ' && character != ' ' && before.is_ascii_digit() != character.is_ascii_digit()
        }) {
            spaced.push(' ');
        }
        spaced.push(character);
        previous = Some(character);
    }
    spaced
        .split(' ')
        .map(|run| {
            run.parse::<usize>()
                .ok()
                .filter(|_| run.bytes().all(|byte| byte.is_ascii_digit()))
                .filter(|_| run == "0" || !run.starts_with('0'))
                .and_then(|number| NUMBERS.get(number).copied())
                .unwrap_or(run)
                .to_owned()
        })
        .collect()
}

mod speech_templates;

fn validate_title(app_id: &str, title: Option<&str>) -> Result<(), String> {
    match title {
        Some(title) if title.trim().is_empty() => Err("title must not be empty".into()),
        Some(_) if app_id == "*" => Err("the global context cannot match a title".into()),
        _ => Ok(()),
    }
}

fn validate_app_id(app_id: &str) -> Result<(), String> {
    if app_id.trim().is_empty() {
        return Err("app_id must not be empty".into());
    }
    Ok(())
}

mod adjuster;
pub use adjuster::Adjuster;
mod sticks;
pub use sticks::{Stick, StickDirection, StickMapping};
