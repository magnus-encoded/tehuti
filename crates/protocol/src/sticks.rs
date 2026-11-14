//! `[sticks]`: stick directions mapped to actions, optionally gated on a held face button.

use super::{Action, ButtonName, MappingAction};

/// An analogue stick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
#[serde(rename_all = "snake_case")]
pub enum Stick {
    Left,
    Right,
}

/// One of the eight 45° sectors a deflected stick points into; screen up is `Up`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
#[serde(rename_all = "kebab-case")]
#[expect(clippy::min_ident_chars, reason = "Up is the direction's name")]
pub enum StickDirection {
    Up,
    UpRight,
    Right,
    DownRight,
    Down,
    DownLeft,
    Left,
    UpLeft,
}

impl StickDirection {
    pub const ALL: [Self; 8] = [
        Self::Up,
        Self::UpRight,
        Self::Right,
        Self::DownRight,
        Self::Down,
        Self::DownLeft,
        Self::Left,
        Self::UpLeft,
    ];

    #[must_use]
    pub const fn is_diagonal(self) -> bool {
        matches!(
            self,
            Self::UpRight | Self::DownRight | Self::DownLeft | Self::UpLeft
        )
    }

    /// The arrow the ticker draws for the direction.
    #[must_use]
    pub const fn arrow(self) -> &'static str {
        match self {
            Self::Up => "\u{2191}",
            Self::UpRight => "\u{2197}",
            Self::Right => "\u{2192}",
            Self::DownRight => "\u{2198}",
            Self::Down => "\u{2193}",
            Self::DownLeft => "\u{2199}",
            Self::Left => "\u{2190}",
            Self::UpLeft => "\u{2196}",
        }
    }
}

/// One `[sticks]` entry as written: an action, an optional modifier and repeat.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct StickEntryFile {
    #[serde(default)]
    with: Option<ButtonName>,
    #[serde(default = "repeat_default")]
    repeat: bool,
    #[serde(flatten)]
    action: MappingAction,
}

const fn repeat_default() -> bool {
    true
}

/// A direction takes one entry or a list of entries with distinct modifiers.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StickEntries {
    One(StickEntryFile),
    Many(Vec<StickEntryFile>),
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private helpers also satisfy unreachable_pub"
)]
pub(crate) type SticksFile =
    std::collections::HashMap<Stick, std::collections::HashMap<StickDirection, toml::Value>>;

/// A validated stick mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickMapping {
    pub stick: Stick,
    pub direction: StickDirection,
    /// The face button that must be held, or `None` for the unmodified layer.
    pub with: Option<ButtonName>,
    pub repeat: bool,
    pub action: Action,
}

const FACE_BUTTONS: [ButtonName; 4] = [
    ButtonName::South,
    ButtonName::East,
    ButtonName::West,
    ButtonName::North,
];

/// Validates `[sticks]`: face-button modifiers, no held keys, one entry per modifier and direction.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private helpers also satisfy unreachable_pub"
)]
pub(crate) fn parse_sticks(file: SticksFile) -> Result<Vec<StickMapping>, String> {
    let mut mappings: Vec<StickMapping> = Vec::new();
    for (stick, directions) in file {
        for (direction, value) in directions {
            let entries: StickEntries = value
                .try_into()
                .map_err(|error: toml::de::Error| format!("sticks entry: {error}"))?;
            let entries = match entries {
                StickEntries::One(entry) => vec![entry],
                StickEntries::Many(entries) => entries,
            };
            for entry in entries {
                let name = format!("{stick:?} {direction:?}").to_lowercase();
                if entry.with.is_some_and(|button| !FACE_BUTTONS.contains(&button)) {
                    return Err(format!("stick {name}: with must be a face button"));
                }
                if matches!(entry.action, MappingAction::Key { hold: true, .. }) {
                    return Err(format!("stick {name} cannot hold a key; hold is for buttons"));
                }
                if mappings.iter().any(|mapping| {
                    mapping.stick == stick && mapping.direction == direction && mapping.with == entry.with
                }) {
                    return Err(format!("stick {name} maps the same modifier twice"));
                }
                mappings.push(StickMapping {
                    stick,
                    direction,
                    with: entry.with,
                    repeat: entry.repeat,
                    action: entry.action.action(),
                });
            }
        }
    }
    Ok(mappings)
}
