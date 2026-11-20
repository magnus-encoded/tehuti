//! Pad identity, physical positions, interaction roles and printed glyphs.

use crate::ticker::{Direction, Piece, Rgb, Shape};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tehuti_protocol::ButtonName;

pub use crate::audio_source::Presence;

/// Where a pad comes from; the tile shows it so the user sees which device is where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    Usb,
    Bluetooth,
    /// A radio dongle that is not Bluetooth.
    Dongle,
    /// A virtual device made by a game-streaming host.
    Streamed,
    /// A virtual device made by a program that could not be named.
    VirtualOther,
    /// Tehuti's own virtual pad.
    Tehuti,
    /// The source could not be told; the tile draws no glyph.
    Unknown,
}

impl Origin {
    /// The short caption line on the pad tile; empty when the source is unknown.
    #[must_use]
    pub const fn caption(self) -> &'static str {
        match self {
            Self::Usb => "USB CABLE",
            Self::Bluetooth => "BLUETOOTH",
            Self::Dongle => "WIRELESS DONGLE",
            Self::Streamed => "STREAMED: SUNSHINE",
            Self::VirtualOther => "VIRTUAL (OTHER APP)",
            Self::Tehuti => "TEHUTI VIRTUAL PAD",
            Self::Unknown => "",
        }
    }

    /// The name of the glyph asset, or none when the source is unknown.
    #[must_use]
    pub const fn glyph(self) -> Option<&'static str> {
        match self {
            Self::Usb => Some("cable"),
            Self::Bluetooth => Some("bluetooth"),
            Self::Dongle => Some("dongle"),
            Self::Streamed => Some("streamed"),
            Self::VirtualOther => Some("virtual"),
            Self::Tehuti => Some("tehuti"),
            Self::Unknown => None,
        }
    }
}

/// A pad's id: stable across reconnects, the same string `ControllerInfo::key` holds today.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PadId(String);

impl PadId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the user chose for one pad; a pad with no saved choice has the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadSettings {
    pub enabled: bool,
    /// The button that records speech on this pad; none means the global default.
    pub push_to_talk: Option<ButtonName>,
}

impl Default for PadSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            push_to_talk: None,
        }
    }
}

/// One controller as the rest of Tehuti sees it: runtime facts only, no settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pad {
    pub id: PadId,
    pub label: String,
    pub model: Identity,
    pub origin: Origin,
    pub presence: Presence,
}

/// What the compositor must do after a pad changes state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PadEffect {
    /// Send a release for each of these buttons.
    ReleaseHeld(Vec<ButtonName>),
    /// Drop whatever the input gate remembers about this pad.
    ForgetGate,
}

/// One pad node as discovery sees it: the facts identifying needs, before any id exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadSighting {
    pub path: PathBuf,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    /// The device's own unique id or physical path; empty when it reports neither.
    pub uniq: String,
    pub origin: Origin,
}

/// Keeps pad ids: owns the identifying rules and the path-to-id memory between sightings.
///
/// Held buttons are one map, button to the paths of the pads holding it: a release is due only
/// when a button's set empties, so a shared button is never released early or twice.
#[derive(Debug, Clone, Default)]
pub struct Pads {
    known: BTreeMap<PathBuf, Pad>,
    held: BTreeMap<ButtonName, BTreeSet<PathBuf>>,
    effects: Vec<(PathBuf, PadEffect)>,
}

impl Pads {
    /// Takes the pads present now, in any order, and returns them in path order.
    ///
    /// Labels count each model independently. A pad's id survives a reconnect: the unique id
    /// or physical path when it is the pad's alone, else the kernel name with an index. A pad
    /// seen on the previous call keeps its id whatever attaches beside it; a new pad takes the
    /// first id of its identity that no pad holds. Pads missing from `sightings` are forgotten.
    pub fn observe(&mut self, sightings: impl IntoIterator<Item = PadSighting>) -> Vec<(PathBuf, Pad)> {
        let mut sightings: Vec<_> = sightings.into_iter().collect();
        sightings.sort_by(|left, right| left.path.cmp(&right.path));
        let mut taken: BTreeSet<String> = sightings
            .iter()
            .filter_map(|sighting| self.known.get(&sighting.path))
            .map(|pad| pad.id.0.clone())
            .collect();
        let mut counts = BTreeMap::<String, usize>::new();
        let mut next = BTreeMap::new();
        let pads = sightings
            .into_iter()
            .map(|sighting| {
                let model = Identity::for_node(sighting.vendor, sighting.product, &sighting.name);
                let model_name = model.model_name(&sighting.name);
                let count = counts.entry(model_name.clone()).or_default();
                *count = count.saturating_add(1);
                let id = self.known.get(&sighting.path).map_or_else(
                    || {
                        let identity = &sighting.uniq;
                        let name = &sighting.name;
                        let candidates = (1_usize..).map(|index| match (identity.is_empty(), index) {
                            (false, 1) => identity.clone(),
                            (false, _) => format!("{identity}#{index}"),
                            (true, _) => format!("{name}#{index}"),
                        });
                        let free = candidates
                            .into_iter()
                            .find(|id| !taken.contains(id))
                            .unwrap_or_default();
                        let _fresh = taken.insert(free.clone());
                        PadId(free)
                    },
                    |pad| pad.id.clone(),
                );
                let pad = Pad {
                    id,
                    label: format!("{model_name} #{count}"),
                    model,
                    origin: sighting.origin,
                    presence: Presence::Present,
                };
                let _old = next.insert(sighting.path.clone(), pad.clone());
                (sighting.path, pad)
            })
            .collect();
        let previous = core::mem::replace(&mut self.known, next);
        self.forget(previous);
        pads
    }

    /// Queues the effects of each pad in `previous` that has vanished.
    fn forget(&mut self, previous: BTreeMap<PathBuf, Pad>) {
        for path in previous.into_keys() {
            if self.known.contains_key(&path) {
                continue;
            }
            let mut released = Vec::new();
            self.held.retain(|button, holders| {
                let _removed = holders.remove(&path);
                if holders.is_empty() {
                    released.push(*button);
                }
                !holders.is_empty()
            });
            if !released.is_empty() {
                self.effects.push((path.clone(), PadEffect::ReleaseHeld(released)));
            }
            self.effects.push((path, PadEffect::ForgetGate));
        }
    }

    /// The id the pad at `path` holds, while it is present.
    #[must_use]
    pub fn id_for(&self, path: &Path) -> Option<&PadId> {
        self.known.get(path).map(|pad| &pad.id)
    }

    /// Records a press or release on the pad at `path`.
    pub fn on_button(&mut self, path: &Path, name: ButtonName, pressed: bool) {
        if !self.known.contains_key(path) {
            return;
        }
        if pressed {
            let _new = self.held.entry(name).or_default().insert(path.to_path_buf());
        } else if let Some(holders) = self.held.get_mut(&name) {
            let _removed = holders.remove(path);
            if holders.is_empty() {
                let _gone = self.held.remove(&name);
            }
        }
    }

    /// The effects of pads that vanished since the last call, each given once.
    pub fn take_effects(&mut self) -> Vec<(PathBuf, PadEffect)> {
        core::mem::take(&mut self.effects)
    }
}

#[cfg(test)]
mod pad_type_tests {
    use super::*;

    #[test]
    fn pad_id_keeps_its_text_and_orders_by_it() {
        assert_eq!(PadId::new("xbox-1").as_str(), "xbox-1");
        assert!(PadId::new("a") < PadId::new("b"));
    }

    #[test]
    fn pad_carries_origin_and_presence() {
        let pad = Pad {
            id: PadId::new("k"),
            label: "PAD #1".into(),
            model: Identity::default(),
            origin: Origin::Bluetooth,
            presence: Presence::Connecting,
        };
        assert_eq!(pad.origin.caption(), "BLUETOOTH");
        assert_eq!(pad.presence.label(), "CONNECTING");
    }
}

#[cfg(test)]
mod origin_tests {
    use super::Origin;

    #[test]
    fn every_known_origin_has_its_own_glyph_and_caption_and_unknown_has_none() {
        let known = [
            Origin::Usb,
            Origin::Bluetooth,
            Origin::Dongle,
            Origin::Streamed,
            Origin::VirtualOther,
            Origin::Tehuti,
        ];
        let glyphs: std::collections::BTreeSet<_> =
            known.iter().filter_map(|origin| origin.glyph()).collect();
        let captions: std::collections::BTreeSet<_> = known.iter().map(|origin| origin.caption()).collect();
        assert_eq!(glyphs.len(), known.len());
        assert_eq!(captions.len(), known.len());
        assert!(captions.iter().all(|caption| !caption.is_empty()));
        assert_eq!(Origin::Unknown.glyph(), None);
        assert_eq!(Origin::Unknown.caption(), "");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Confirm,
    Back,
    Keyboard,
}

/// Device facts resolved together at the input boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Identity {
    pub family: PadFamily,
    pub swap_north_west: bool,
    model: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
struct Profile {
    vendor: u16,
    identity: Identity,
    roles: [ButtonName; 3],
    faces: &'static [FaceGlyph],
    labels: &'static [(ButtonName, &'static str)],
}

const STANDARD_ROLES: [ButtonName; 3] = [ButtonName::South, ButtonName::East, ButtonName::North];
const FALLBACK: Profile = Profile {
    vendor: 0,
    identity: Identity {
        family: PadFamily::Letters,
        swap_north_west: false,
        model: None,
    },
    roles: STANDARD_ROLES,
    faces: &LETTER_FACES,
    labels: LETTER_LABELS,
};
/// The Wii remote's USB-style id and the name of its buttons node. Its Classic Controller
/// extension reuses the id under a longer name, so the name is part of the match.
const WII_REMOTE_ID: (u16, u16) = (0x057e, 0x0306);
const WII_REMOTE_NODE: &str = "Nintendo Wii Remote";
/// Held sideways and used alone, 1 and 2 are the pad: 1 confirms, 2 goes back, A opens the
/// keyboard and + carries push-to-talk as Record. The input layer remaps the buttons onto the
/// standard positions, so the standard roles apply. B is left unmapped.
const REMOTE: Profile = Profile {
    vendor: 0x057e,
    identity: Identity {
        family: PadFamily::Remote,
        swap_north_west: false,
        model: Some("WII REMOTE"),
    },
    roles: STANDARD_ROLES,
    faces: &REMOTE_FACES,
    labels: REMOTE_LABELS,
};
/// With a nunchuk or another pad attached, A confirms and B goes back, 1 opens the keyboard and
/// + still carries push-to-talk. 2 is left unmapped.
const REMOTE_PAIRED: Profile = Profile {
    vendor: 0x057e,
    identity: Identity {
        family: PadFamily::RemotePaired,
        swap_north_west: false,
        model: Some("WII REMOTE"),
    },
    roles: STANDARD_ROLES,
    faces: &REMOTE_PAIRED_FACES,
    labels: REMOTE_LABELS,
};
const PROFILES: [Profile; 3] = [
    Profile {
        vendor: 0x057e,
        identity: Identity {
            family: PadFamily::Nintendo,
            swap_north_west: false,
            model: None,
        },
        roles: [ButtonName::East, ButtonName::South, ButtonName::North],
        faces: &NINTENDO_FACES,
        labels: NINTENDO_LABELS,
    },
    Profile {
        vendor: 0x054c,
        identity: Identity {
            family: PadFamily::Symbols,
            swap_north_west: false,
            model: None,
        },
        roles: STANDARD_ROLES,
        faces: &SYMBOL_FACES,
        labels: SYMBOL_LABELS,
    },
    Profile {
        vendor: 0x045e,
        identity: Identity {
            family: PadFamily::Letters,
            swap_north_west: true,
            model: None,
        },
        roles: STANDARD_ROLES,
        faces: &LETTER_FACES,
        labels: LETTER_LABELS,
    },
];

impl Identity {
    #[must_use]
    pub fn for_device(vendor: u16, product: u16) -> Self {
        let mut identity = PROFILES
            .iter()
            .find(|profile| profile.vendor == vendor)
            .unwrap_or(&FALLBACK)
            .identity;
        identity.model = match (vendor, product) {
            (0x054c, 0x05c4 | 0x09cc) => Some("DUALSHOCK 4"),
            (0x054c, 0x0ce6) => Some("DUALSENSE"),
            (0x054c, 0x0268) => Some("DUALSHOCK 3"),
            (0x045e, _) => Some("XBOX CONTROLLER"),
            (0x057e, 0x2009) => Some("SWITCH PRO"),
            (0x057e, 0x2006 | 0x2007) => Some("JOY-CON"),
            _ => None,
        };
        identity
    }

    /// The same pad once a nunchuk or another pad is attached beside it, or alone again.
    #[must_use]
    pub const fn with_companion(mut self, companion: bool) -> Self {
        self.family = match (self.family, companion) {
            (PadFamily::Remote | PadFamily::RemotePaired, true) => PadFamily::RemotePaired,
            (PadFamily::Remote | PadFamily::RemotePaired, false) => PadFamily::Remote,
            (family, _) => family,
        };
        self
    }

    /// Like [`Self::for_device`], with the node name that tells a Wii remote's buttons node
    /// from the extension nodes that share its id.
    #[must_use]
    pub fn for_node(vendor: u16, product: u16, name: &str) -> Self {
        if (vendor, product) == WII_REMOTE_ID && name == WII_REMOTE_NODE {
            return REMOTE.identity;
        }
        Self::for_device(vendor, product)
    }

    #[must_use]
    pub fn model_name(self, kernel_name: &str) -> String {
        self.model
            .map_or_else(|| kernel_name.to_uppercase(), str::to_owned)
    }
}

/// The button names and role positions of a pad.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PadFamily {
    #[default]
    Letters,
    Symbols,
    Nintendo,
    Remote,
    RemotePaired,
}

impl PadFamily {
    /// True for a Wii remote in either layout.
    #[must_use]
    pub const fn is_remote(self) -> bool {
        matches!(self, Self::Remote | Self::RemotePaired)
    }

    #[must_use]
    pub fn button(self, role: Role) -> ButtonName {
        let profile = self.profile();
        let [confirm, back, keyboard] = profile.roles;
        match role {
            Role::Confirm => confirm,
            Role::Back => back,
            Role::Keyboard => keyboard,
        }
    }

    #[must_use]
    pub fn glyph(self, role: Role) -> Piece {
        self.piece(self.button(role))
    }

    fn profile(self) -> &'static Profile {
        match self {
            Self::Remote => return &REMOTE,
            Self::RemotePaired => return &REMOTE_PAIRED,
            Self::Letters | Self::Symbols | Self::Nintendo => {}
        }
        PROFILES
            .iter()
            .find(|profile| profile.identity.family == self)
            .unwrap_or(&FALLBACK)
    }

    fn face(self, button: ButtonName) -> Option<&'static FaceGlyph> {
        self.profile().faces.iter().find(|face| face.button == button)
    }

    #[must_use]
    pub fn label(self, button: ButtonName) -> &'static str {
        self.face(button)
            .map(|face| face.label)
            .or_else(|| {
                self.profile()
                    .labels
                    .iter()
                    .find(|(position, _)| *position == button)
                    .map(|(_, label)| *label)
            })
            .or_else(|| {
                COMMON_LABELS
                    .iter()
                    .find(|(position, _)| *position == button)
                    .map(|(_, label)| *label)
            })
            .unwrap_or("Button")
    }

    /// The button's printed symbol, D-pad arm or label chip.
    #[must_use]
    pub fn piece(self, button: ButtonName) -> Piece {
        if let Some(face) = self.face(button) {
            return Piece::Glyph {
                shape: face.shape,
                colour: face.colour,
            };
        }
        if let Some((_, direction)) = DPAD_SHAPES.iter().find(|(position, _)| *position == button) {
            return Piece::Glyph {
                shape: Shape::Dpad(*direction),
                colour: Rgb::WHITE,
            };
        }
        Piece::Chip(self.label(button).into())
    }
}

#[derive(Debug)]
struct FaceGlyph {
    button: ButtonName,
    label: &'static str,
    shape: Shape,
    colour: Rgb,
}

const LETTER_FACES: [FaceGlyph; 6] = [
    FaceGlyph {
        button: ButtonName::South,
        label: "A",
        shape: Shape::Disc('A'),
        colour: Rgb(76, 175, 80),
    },
    FaceGlyph {
        button: ButtonName::East,
        label: "B",
        shape: Shape::Disc('B'),
        colour: Rgb(229, 57, 53),
    },
    FaceGlyph {
        button: ButtonName::West,
        label: "X",
        shape: Shape::Disc('X'),
        colour: Rgb(30, 136, 229),
    },
    FaceGlyph {
        button: ButtonName::North,
        label: "Y",
        shape: Shape::Disc('Y'),
        colour: Rgb(253, 216, 53),
    },
    FaceGlyph {
        button: ButtonName::Select,
        label: "View",
        shape: Shape::View,
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::Start,
        label: "Menu",
        shape: Shape::Menu,
        colour: Rgb::WHITE,
    },
];

const NINTENDO_FACES: [FaceGlyph; 4] = [
    FaceGlyph {
        button: ButtonName::East,
        label: "A",
        shape: Shape::Disc('A'),
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::South,
        label: "B",
        shape: Shape::Disc('B'),
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::North,
        label: "X",
        shape: Shape::Disc('X'),
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::West,
        label: "Y",
        shape: Shape::Disc('Y'),
        colour: Rgb::WHITE,
    },
];

const NINTENDO_LABELS: &[(ButtonName, &str)] = &[
    (ButtonName::LeftBumper, "L"),
    (ButtonName::RightBumper, "R"),
    (ButtonName::LeftTrigger, "ZL"),
    (ButtonName::RightTrigger, "ZR"),
    (ButtonName::Select, "−"),
    (ButtonName::Start, "+"),
    (ButtonName::Record, "Capture"),
    (ButtonName::LeftStick, "LS"),
    (ButtonName::RightStick, "RS"),
];

const REMOTE_FACES: [FaceGlyph; 3] = [
    FaceGlyph {
        button: ButtonName::South,
        label: "1",
        shape: Shape::Disc('1'),
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::East,
        label: "2",
        shape: Shape::Disc('2'),
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::North,
        label: "A",
        shape: Shape::Disc('A'),
        colour: Rgb::WHITE,
    },
];

const REMOTE_PAIRED_FACES: [FaceGlyph; 3] = [
    FaceGlyph {
        button: ButtonName::South,
        label: "A",
        shape: Shape::Disc('A'),
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::East,
        label: "B",
        shape: Shape::Disc('B'),
        colour: Rgb::WHITE,
    },
    FaceGlyph {
        button: ButtonName::North,
        label: "1",
        shape: Shape::Disc('1'),
        colour: Rgb::WHITE,
    },
];

const REMOTE_LABELS: &[(ButtonName, &str)] = &[
    (ButtonName::Select, "−"),
    (ButtonName::Record, "+"),
    (ButtonName::Guide, "Home"),
];

const SYMBOL_FACES: [FaceGlyph; 4] = [
    FaceGlyph {
        button: ButtonName::South,
        label: "Cross",
        shape: Shape::Cross,
        colour: Rgb(124, 178, 232),
    },
    FaceGlyph {
        button: ButtonName::East,
        label: "Circle",
        shape: Shape::Ring,
        colour: Rgb(255, 102, 102),
    },
    FaceGlyph {
        button: ButtonName::West,
        label: "Square",
        shape: Shape::Square,
        colour: Rgb(255, 105, 248),
    },
    FaceGlyph {
        button: ButtonName::North,
        label: "Triangle",
        shape: Shape::Triangle,
        colour: Rgb(64, 226, 160),
    },
];

const LETTER_LABELS: &[(ButtonName, &str)] = &[
    (ButtonName::LeftBumper, "LB"),
    (ButtonName::RightBumper, "RB"),
    (ButtonName::LeftTrigger, "LT"),
    (ButtonName::RightTrigger, "RT"),
    (ButtonName::Record, "Share"),
    (ButtonName::LeftStick, "LS"),
    (ButtonName::RightStick, "RS"),
];

const SYMBOL_LABELS: &[(ButtonName, &str)] = &[
    (ButtonName::LeftBumper, "L1"),
    (ButtonName::RightBumper, "R1"),
    (ButtonName::LeftTrigger, "L2"),
    (ButtonName::RightTrigger, "R2"),
    (ButtonName::Select, "Share"),
    (ButtonName::Start, "Options"),
    (ButtonName::Record, "Record"),
    (ButtonName::LeftStick, "L3"),
    (ButtonName::RightStick, "R3"),
];

const COMMON_LABELS: &[(ButtonName, &str)] = &[
    (ButtonName::Paddle1, "P1"),
    (ButtonName::Paddle2, "P2"),
    (ButtonName::Paddle3, "P3"),
    (ButtonName::Paddle4, "P4"),
    (ButtonName::Guide, "Guide"),
    (ButtonName::DpadUp, "Up"),
    (ButtonName::DpadDown, "Down"),
    (ButtonName::DpadLeft, "Left"),
    (ButtonName::DpadRight, "Right"),
];
const DPAD_SHAPES: &[(ButtonName, Direction)] = &[
    (ButtonName::DpadUp, Direction::Upward),
    (ButtonName::DpadDown, Direction::Down),
    (ButtonName::DpadLeft, Direction::Left),
    (ButtonName::DpadRight, Direction::Right),
];

#[cfg(test)]
mod pads_tests {
    use super::*;
    use proptest::prelude::*;

    fn sighting(path: &str, name: &str, uniq: &str) -> PadSighting {
        PadSighting {
            path: path.into(),
            name: name.into(),
            vendor: 0x045e,
            product: 0x0b13,
            uniq: uniq.into(),
            origin: Origin::Usb,
        }
    }

    fn ids(pads: &[(PathBuf, Pad)]) -> Vec<&str> {
        pads.iter().map(|(_, pad)| pad.id.as_str()).collect()
    }

    #[test]
    fn pads_of_different_names_sharing_an_identity_get_different_ids() {
        let pads = Pads::default().observe([
            sighting("/dev/input/event1", "Pad A", "shared"),
            sighting("/dev/input/event2", "Pad B", "shared"),
        ]);
        assert_ne!(pads[0].1.id, pads[1].1.id);
    }

    #[test]
    fn a_duplicate_attaching_never_reassigns_the_pad_already_there() {
        let mut pads = Pads::default();
        let original = sighting("/dev/input/event5", "Pad", "shared");
        let before = pads.observe([original.clone()]);
        // The newcomer's node sorts before the original's.
        let after = pads.observe([sighting("/dev/input/event2", "Pad", "shared"), original]);
        assert_eq!(after[1].0, PathBuf::from("/dev/input/event5"));
        assert_eq!(after[1].1.id, before[0].1.id);
        assert_ne!(after[0].1.id, after[1].1.id);
    }

    #[test]
    fn identical_pads_without_ids_take_indexed_name_ids() {
        let pads = Pads::default().observe([
            sighting("/dev/input/event1", "Pad", ""),
            sighting("/dev/input/event2", "Pad", ""),
        ]);
        assert_eq!(ids(&pads), ["Pad#1", "Pad#2"]);
    }

    #[test]
    fn matching_models_are_numbered_by_device_path() {
        let mut first = sighting("/dev/input/event9", "Wireless Controller", "");
        let mut second = sighting("/dev/input/event2", "Wireless Controller", "");
        (first.vendor, first.product) = (0x054c, 0x05c4);
        (second.vendor, second.product) = (0x054c, 0x09cc);
        let pads = Pads::default().observe([first, second]);
        let labels: Vec<_> = pads.iter().map(|(_, pad)| pad.label.as_str()).collect();
        assert_eq!(labels, ["DUALSHOCK 4 #1", "DUALSHOCK 4 #2"]);
    }

    #[test]
    fn an_origin_reaches_the_pad() {
        let mut seen = sighting("/dev/input/event9", "Xbox Wireless Controller", "");
        seen.origin = Origin::Bluetooth;
        let pads = Pads::default().observe([seen]);
        assert_eq!(pads[0].1.origin, Origin::Bluetooth);
    }

    proptest! {
        /// Each step is the set of nodes present after a sighting or a drop.
        #[test]
        fn any_interleaving_of_sightings_and_drops_keeps_present_keys_unique(
            steps in proptest::collection::vec(
                proptest::collection::btree_map(0_u8..6, (0_u8..3, any::<bool>()), 0..6),
                1..12,
            ),
        ) {
            let mut pads = Pads::default();
            for step in steps {
                let present = step.into_iter().map(|(node, (name, has_id))| {
                    let uniq = if has_id { format!("id{}", name % 2) } else { String::new() };
                    sighting(&format!("/dev/input/event{node}"), &format!("Pad{name}"), &uniq)
                });
                let pads = pads.observe(present);
                let unique: BTreeSet<_> = pads.iter().map(|(_, pad)| &pad.id).collect();
                prop_assert_eq!(unique.len(), pads.len());
            }
        }
    }
}
