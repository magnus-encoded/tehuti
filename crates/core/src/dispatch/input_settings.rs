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

/// A connected pad's saved kernel name and its visible, numbered label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerInfo {
    pub name: String,
    pub label: String,
}

impl From<&str> for ControllerInfo {
    fn from(name: &str) -> Self {
        Self {
            name: name.into(),
            label: format!("{} #1", name.to_uppercase()),
        }
    }
}

impl From<String> for ControllerInfo {
    fn from(name: String) -> Self {
        Self {
            label: format!("{} #1", name.to_uppercase()),
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
        } else if mentions(&self.name, &["dualshock", "dualsense", "playstation", "wireless controller", "ps4", "ps5"]) {
            PadKind::PlayStation
        } else if mentions(&self.name, &["nintendo", "switch", "joy-con", "pro controller"]) {
            PadKind::Switch
        } else {
            PadKind::Generic
        }
    }

    /// The name voice uses for this pad, and the caption under its tile.
    #[must_use]
    pub fn spoken_name(&self) -> String {
        tehuti_protocol::normalize_phrase(&self.name)
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
}

/// The Input page's rows of tiles, top to bottom.
pub const CONTROLLER_ROW: usize = 0;
/// The row of settings for the controller that has focus: ENABLED, then the push-to-talk tiles.
pub const CONTROLLER_SETTINGS_ROW: usize = 1;
pub const MICROPHONE_ROW: usize = 2;

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
        if mentions(&text, &["rtp", "network", "tunnel", "stream", "raop"]) {
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
    pub fn rows(&self) -> [usize; 3] {
        [
            self.controllers.len(),
            if self.controllers.is_empty() {
                0
            } else {
                TALK_OPTIONS.len().saturating_add(1)
            },
            self.microphone_options().len(),
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
            .and_then(|pad| self.controller_bindings.get(&pad.name))
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
                    |microphone| microphone.description.to_uppercase(),
                ),
        }
    }
}
