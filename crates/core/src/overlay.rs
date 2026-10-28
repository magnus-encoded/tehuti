//! Pad ownership and composition order, from the human-facing top layer down.

/// A layer in the overlay stack. Renderers and dispatch traverse the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Cursor,
    Settings,
    SystemMenu,
    Secret,
    SecretUnavailable,
    Notice,
    MouseNotice,
    Bar,
    Keyboard,
    Mouse,
    Application,
}

/// Topmost first. Spatial placement of controls follows ADR 0005.
pub const STACK: [Overlay; 11] = [
    Overlay::Cursor,
    Overlay::Settings,
    Overlay::SystemMenu,
    Overlay::Secret,
    Overlay::SecretUnavailable,
    Overlay::Notice,
    Overlay::MouseNotice,
    Overlay::Bar,
    Overlay::Keyboard,
    Overlay::Mouse,
    Overlay::Application,
];

impl Overlay {
    /// The layers that surrender held input when this overlay opens.
    pub fn below(self) -> impl Iterator<Item = Self> {
        STACK.into_iter().skip_while(move |layer| *layer != self).skip(1)
    }
}
