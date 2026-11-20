//! The Input page of the settings view: one tile per controller, microphone and push-to-talk button.

use super::Effect;
use crate::audio_source::AudioSource;
use crate::pad::Origin;
use crate::tiles::Focus;

/// Which connected pad drives Tehuti.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ControllerChoice {
    /// Every connected pad.
    #[default]
    Auto,
    /// Only pads with this device name, while one is connected.
    Named(String),
}

/// A connected pad: its kernel name, a key that stays the same across reconnects, and its numbered label.
///
/// Settings kept per pad (enabled, push-to-talk) are keyed by `key`: the device's
/// unique id when it has one, else its physical path, else its name and an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerInfo {
    pub name: String,
    pub key: String,
    pub label: String,
    pub origin: Origin,
}

impl From<&str> for ControllerInfo {
    fn from(name: &str) -> Self {
        Self {
            name: name.into(),
            key: name.into(),
            label: format!("{} #1", name.to_uppercase()),
            origin: Origin::default(),
        }
    }
}

impl From<String> for ControllerInfo {
    fn from(name: String) -> Self {
        Self {
            label: format!("{} #1", name.to_uppercase()),
            key: name.clone(),
            origin: Origin::default(),
            name,
        }
    }
}

/// The drawing a controller tile shows, guessed from the pad's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadKind {
    Xbox,
    PlayStation,
    Switch,
    Generic,
}

/// The drawing a microphone tile shows, guessed from the source's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicKind {
    Headset,
    Usb,
    /// A sound chip: analog or built-in capture.
    Builtin,
    /// A stream from the network, such as RTP.
    Network,
    Phone,
    Generic,
}

fn mentions(text: &str, words: &[&str]) -> bool {
    let lowered = text.to_lowercase();
    words.iter().any(|word| lowered.contains(word))
}

impl ControllerInfo {
    #[must_use]
    pub fn kind(&self) -> PadKind {
        if mentions(&self.name, &["xbox", "x-box"]) {
            PadKind::Xbox
        } else if mentions(
            &self.name,
            &[
                "dualshock",
                "dualsense",
                "playstation",
                "wireless controller",
                "ps4",
                "ps5",
            ],
        ) {
            PadKind::PlayStation
        } else if mentions(&self.name, &["nintendo", "switch", "joy-con", "pro controller"]) {
            PadKind::Switch
        } else {
            PadKind::Generic
        }
    }

    /// The name voice uses for this pad, and the caption under its tile: the numbered label,
    /// so pads of one model differ. The bare kernel name works too while it is unique.
    #[must_use]
    pub fn spoken_name(&self) -> String {
        tehuti_protocol::normalize_phrase(&self.label)
    }
}

/// Which capture sources push-to-talk records.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MicrophoneChoice {
    /// Every capture source at once; the loudest recording wins.
    #[default]
    All,
    /// The `PipeWire` default source.
    Auto,
    /// One `PipeWire` node.
    Named(String),
}

/// A `PipeWire` capture source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    /// The `node.name` that the speech sidecar targets.
    pub name: String,
    /// The `node.description` shown to the user.
    pub description: String,
    /// Set when this source is the `AudioRelay` player's: whether a phone is connected.
    pub relay: Option<RelayState>,
}

/// The connection of an `AudioRelay` source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayState {
    /// A phone is sending sound.
    Connected,
    /// The player listens and no phone has connected.
    Waiting,
}

impl RelayState {
    /// The word shown under the tile.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connected => "CONNECTED",
            Self::Waiting => "WAITING",
        }
    }
}

/// The Input page's rows of tiles, top to bottom.
pub const CONTROLLER_ROW: usize = 0;
/// The row of settings for the controller that has focus: ENABLED, then the push-to-talk tiles.
pub const CONTROLLER_SETTINGS_ROW: usize = 1;
pub const MICROPHONE_ROW: usize = 2;
/// Hosts that may send a remote microphone; confirm connects to one.
pub const REMOTE_ROW: usize = 3;

/// The push-to-talk tiles of a controller's row, after ENABLED: the context's own button, then the two a pad can lend.
pub const TALK_OPTIONS: [Option<tehuti_protocol::ButtonName>; 3] = [
    None,
    Some(tehuti_protocol::ButtonName::Record),
    Some(tehuti_protocol::ButtonName::Start),
];

/// The stick deadzones a pad's DEADZONE tile steps through, in thousandths of full travel.
pub const DEADZONE_STEPS: [u16; 8] = [0, 50, 100, 150, 200, 250, 300, 350];
/// The deadzone of a pad with no saved choice: the 15% Tehuti always applied to its own stick reading.
pub const DEFAULT_DEADZONE: u16 = 150;

impl Microphone {
    #[must_use]
    pub fn kind(&self) -> MicKind {
        let text = format!("{} {}", self.name, self.description);
        if self.relay.is_some() {
            MicKind::Phone
        } else if mentions(&text, &["rtp", "network", "tunnel", "stream", "raop"]) {
            MicKind::Network
        } else if mentions(&text, &["phone-", "smartphone", "android", "iphone", "phone mic"]) {
            MicKind::Phone
        } else if mentions(&text, &["headset", "headphone", "bluez", "arctis", "hands-free"]) {
            MicKind::Headset
        } else if mentions(&text, &["usb"]) {
            MicKind::Usb
        } else if mentions(&text, &["analog", "built-in", "builtin", "pci", "internal"]) {
            MicKind::Builtin
        } else {
            MicKind::Generic
        }
    }

    /// This node as an audio source: a microphone or a stream, and whether it is usable yet.
    #[must_use]
    pub fn source(&self) -> AudioSource {
        use crate::audio_source::Presence;
        let relay = self.relay.map(|state| match state {
            RelayState::Connected => Presence::Present,
            RelayState::Waiting => Presence::Connecting,
        });
        AudioSource::classify(&self.name, &self.description, relay)
    }

    /// The name voice uses for this microphone, and the caption under its tile.
    #[must_use]
    pub fn spoken_name(&self) -> String {
        tehuti_protocol::normalize_phrase(&self.description)
    }

    /// The text under the tile: the connection of an `AudioRelay` source, else the spoken name.
    #[must_use]
    pub fn caption(&self) -> String {
        self.relay
            .map_or_else(|| self.spoken_name(), |state| state.label().to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputSettings {
    pub focus: Focus,
    /// The controller tile whose settings row is open below it.
    pub controller_cursor: usize,
    /// Connected pads: first seen first, late arrivals at the end.
    pub controllers: Vec<ControllerInfo>,
    /// The saved single-pad choice from before per-controller tiles.
    pub controller: ControllerChoice,
    /// Capture sources: first seen first, late arrivals at the end.
    pub microphones: Vec<Microphone>,
    pub microphone: MicrophoneChoice,
    pub active_controller: Option<String>,
    /// What the user chose per pad; a pad with no entry has the defaults.
    pub pads: std::collections::BTreeMap<crate::pad::PadId, crate::pad::PadSettings>,
    /// Saved stick deadzones by pad key, in thousandths; a pad without one uses the default.
    pub controller_deadzones: std::collections::BTreeMap<String, u16>,
    /// Where each pad's sticks rest, by pad key: X, Y, right X, right Y, in thousandths of half travel.
    pub controller_recentres: std::collections::BTreeMap<String, [i16; 4]>,
    /// Microphone node names the user turned off; never recorded, also under ALL.
    pub disabled_microphones: Vec<String>,
    /// Hosts offered for a remote microphone, as the tailnet reported them; empty until an answer comes.
    pub remote_hosts: Vec<String>,
    /// The in-use pad whose disabling waits for a second confirm.
    pub pending_disable: Option<String>,
}

impl InputSettings {
    /// The controller whose tile or settings row has focus.
    #[must_use]
    pub fn selected_controller(&self) -> Option<&ControllerInfo> {
        self.controllers.get(self.controller_cursor)
    }

    /// The tiles in each row; the controller rows may be empty, the microphone row never is.
    #[must_use]
    pub fn rows(&self) -> [usize; 4] {
        [
            self.controllers.len(),
            if self.controllers.is_empty() {
                0
            } else {
                TALK_OPTIONS.len().saturating_add(3)
            },
            self.microphone_options().len(),
            self.remote_hosts.len(),
        ]
    }

    /// The stick deadzone of the pad with this key, in thousandths of full travel.
    #[must_use]
    pub fn deadzone(&self, key: &str) -> u16 {
        self.controller_deadzones
            .get(key)
            .copied()
            .unwrap_or(DEFAULT_DEADZONE)
    }

    /// The next deadzone after the pad's current one, wrapping round; a value off the steps moves to the next larger step.
    #[must_use]
    pub fn next_deadzone(&self, key: &str) -> u16 {
        let current = self.deadzone(key);
        DEADZONE_STEPS
            .iter()
            .copied()
            .find(|step| *step > current)
            .unwrap_or(DEADZONE_STEPS[0])
    }

    /// The saved choices of the pad with this key, or the defaults.
    #[must_use]
    pub fn pad_settings(&self, key: &str) -> crate::pad::PadSettings {
        self.pads
            .get(&crate::pad::PadId::new(key))
            .copied()
            .unwrap_or_default()
    }

    /// Stores a pad's choices; the defaults are stored as no entry.
    pub(super) fn store_pad_settings(&mut self, key: &str, settings: crate::pad::PadSettings) {
        let id = crate::pad::PadId::new(key);
        if settings == crate::pad::PadSettings::default() {
            let _removed = self.pads.remove(&id);
        } else {
            let _previous = self.pads.insert(id, settings);
        }
    }

    #[must_use]
    pub fn controller_enabled(&self, name: &str) -> bool {
        self.pad_settings(name).enabled
    }

    /// Keys of the pads the user turned off, in key order.
    #[must_use]
    pub fn disabled_controllers(&self) -> Vec<String> {
        self.pads
            .iter()
            .filter(|(_, settings)| !settings.enabled)
            .map(|(id, _)| id.as_str().to_owned())
            .collect()
    }

    /// The push-to-talk buttons chosen per pad key.
    #[must_use]
    pub fn controller_bindings(&self) -> std::collections::BTreeMap<String, tehuti_protocol::ButtonName> {
        self.pads
            .iter()
            .filter_map(|(id, settings)| {
                settings
                    .push_to_talk
                    .map(|button| (id.as_str().to_owned(), button))
            })
            .collect()
    }

    /// Replaces every pad's push-to-talk choice with `bindings`; on/off choices stay.
    pub(super) fn restore_bindings(
        &mut self,
        bindings: &std::collections::BTreeMap<String, tehuti_protocol::ButtonName>,
    ) {
        let keys: Vec<String> = self
            .pads
            .keys()
            .map(|id| id.as_str().to_owned())
            .chain(bindings.keys().cloned())
            .collect();
        for key in keys {
            let mut settings = self.pad_settings(&key);
            settings.push_to_talk = bindings.get(&key).copied();
            self.store_pad_settings(&key, settings);
        }
    }

    /// Replaces every pad's on/off choice: exactly `disabled` are off; push-to-talk choices stay.
    pub(super) fn restore_disabled(&mut self, disabled: &[String]) {
        let keys: Vec<String> = self
            .pads
            .keys()
            .map(|id| id.as_str().to_owned())
            .chain(disabled.iter().cloned())
            .collect();
        for key in keys {
            let mut settings = self.pad_settings(&key);
            settings.enabled = !disabled.contains(&key);
            self.store_pad_settings(&key, settings);
        }
    }

    /// The named capture node as an audio source; a name that is not listed is Gone.
    #[must_use]
    pub fn audio_source(&self, name: &str) -> AudioSource {
        self.microphones
            .iter()
            .find(|microphone| microphone.name == name)
            .map_or_else(|| AudioSource::gone(name), Microphone::source)
    }

    #[must_use]
    pub fn microphone_enabled(&self, name: &str) -> bool {
        !self.disabled_microphones.iter().any(|disabled| disabled == name)
    }

    /// Records whether the microphone `name` is used; the effect carries the change out.
    pub(super) fn set_microphone_enabled(&mut self, name: String, enabled: bool) -> Effect {
        self.disabled_microphones.retain(|disabled| *disabled != name);
        if !enabled {
            self.disabled_microphones.push(name.clone());
        }
        Effect::SettingsMicrophoneEnabled { name, enabled }
    }

    /// Whether `name` is the pad the user is driving Tehuti with right now.
    #[must_use]
    pub fn controller_in_use(&self, name: &str) -> bool {
        self.active_controller.as_deref() == Some(name)
    }

    #[must_use]
    pub fn focused_controller(&self) -> Option<&ControllerInfo> {
        (self.focus.row == CONTROLLER_ROW)
            .then(|| self.controllers.get(self.focus.col))
            .flatten()
    }

    #[must_use]
    pub fn focused_microphone(&self) -> Option<MicrophoneChoice> {
        (self.focus.row == MICROPHONE_ROW)
            .then(|| self.microphone_options().get(self.focus.col).cloned())
            .flatten()
    }

    /// Records whether `name` drives Tehuti; the effect carries the change out.
    pub(super) fn set_controller_enabled(&mut self, name: String, enabled: bool) -> Effect {
        let mut settings = self.pad_settings(&name);
        settings.enabled = enabled;
        self.store_pad_settings(&name, settings);
        self.pending_disable = None;
        Effect::SettingsControllerEnabled { name, enabled }
    }

    /// The push-to-talk tile chosen for the pad whose row is open.
    #[must_use]
    pub fn talk_choice(&self) -> Option<tehuti_protocol::ButtonName> {
        self.selected_controller()
            .and_then(|pad| self.pad_settings(&pad.key).push_to_talk)
    }

    #[must_use]
    pub fn talk_label(option: Option<tehuti_protocol::ButtonName>) -> &'static str {
        if option == Some(tehuti_protocol::ButtonName::Start) {
            "MENU"
        } else if option == Some(tehuti_protocol::ButtonName::Record) {
            "SHARE"
        } else {
            "CONTEXT"
        }
    }

    /// All, Auto, then each listed source, then a saved source that is not listed.
    #[must_use]
    pub fn microphone_options(&self) -> Vec<MicrophoneChoice> {
        let mut options = vec![MicrophoneChoice::All, MicrophoneChoice::Auto];
        for microphone in &self.microphones {
            let option = MicrophoneChoice::Named(microphone.name.clone());
            if !options.contains(&option) {
                options.push(option);
            }
        }
        if !options.contains(&self.microphone) {
            options.push(self.microphone.clone());
        }
        options
    }

    #[must_use]
    pub fn microphone_tile_label(&self, choice: &MicrophoneChoice) -> String {
        match choice {
            MicrophoneChoice::All => "ALL".into(),
            MicrophoneChoice::Auto => "AUTO".into(),
            MicrophoneChoice::Named(name) => self
                .microphones
                .iter()
                .find(|microphone| microphone.name == *name)
                .map_or_else(
                    || name.to_uppercase(),
                    |microphone| {
                        if microphone.relay.is_some() {
                            "AUDIORELAY".to_owned()
                        } else {
                            microphone.description.to_uppercase()
                        }
                    },
                ),
        }
    }
}
