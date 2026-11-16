//! Pad identity, physical positions, interaction roles and printed glyphs.

use crate::ticker::{Direction, Piece, Rgb, Shape};
use tehuti_protocol::ButtonName;

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
