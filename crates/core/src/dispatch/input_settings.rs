//! The Input page of the settings view: one tile per controller, microphone and push-to-talk button.

use super::Effect;
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
    pub connection: Connection,
}

/// How a pad reaches the machine; the tile shows it so identical pads can be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connection {
    #[default]
    Usb,
    Bluetooth,
    /// A radio dongle that is not Bluetooth.
    Dongle,
}

impl From<&str> for ControllerInfo {
    fn from(name: &str) -> Self {
        Self {
            name: name.into(),
            key: name.into(),
            label: format!("{} #1", name.to_uppercase()),
            connection: Connection::default(),
        }
    }
}

impl From<String> for ControllerInfo {
    fn from(name: String) -> Self {
        Self {
            label: format!("{} #1", name.to_uppercase()),
            key: name.clone(),
            connection: Connection::default(),
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
    pub controller_bindings: std::collections::BTreeMap<String, tehuti_protocol::ButtonName>,
    /// Pad names the user turned off.
    pub disabled_controllers: Vec<String>,
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
                TALK_OPTIONS.len().saturating_add(1)
            },
            self.microphone_options().len(),
            self.remote_hosts.len(),
        ]
    }

    #[must_use]
    pub fn controller_enabled(&self, name: &str) -> bool {
        !self.disabled_controllers.iter().any(|disabled| disabled == name)
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
        self.disabled_controllers.retain(|disabled| *disabled != name);
        if !enabled {
            self.disabled_controllers.push(name.clone());
        }
        self.pending_disable = None;
        Effect::SettingsControllerEnabled { name, enabled }
    }

    /// The push-to-talk tile chosen for the pad whose row is open.
    #[must_use]
    pub fn talk_choice(&self) -> Option<tehuti_protocol::ButtonName> {
        self.selected_controller()
            .and_then(|pad| self.controller_bindings.get(&pad.key))
            .copied()
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
