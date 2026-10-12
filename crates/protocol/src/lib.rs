//! Protocol v0 types shared by the core and plugins.

/// A controller button. Face buttons are named by position: South is A on an Xbox pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonName {
    South,
    East,
    North,
    West,
    LeftBumper,
    RightBumper,
    Select,
    Start,
    Guide,
    LeftStick,
    RightStick,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

impl ButtonName {
    pub const ALL: [Self; 15] = [
        Self::South,
        Self::East,
        Self::North,
        Self::West,
        Self::LeftBumper,
        Self::RightBumper,
        Self::Select,
        Self::Start,
        Self::Guide,
        Self::LeftStick,
        Self::RightStick,
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonState {
    Pressed,
    Released,
}

/// Input that reaches the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Button { name: ButtonName, state: ButtonState },
}

/// A key the core can send to the focused client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyName {
    Enter,
}

/// What the core asks its edges to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Press and release `key` in the focused client.
    Key { key: KeyName },
}
