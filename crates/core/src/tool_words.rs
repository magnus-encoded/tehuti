//! Built-in tool words: spoken in any context, matched before plugin routing.

/// Spoken phrase that opens the global help overlay.
pub const WHAT_CAN_I_SAY: &str = "what can i say";

/// A built-in tool a spoken word opens or toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolWord {
    /// Opens the on-screen keyboard.
    Keyboard,
    /// Toggles pointer mode.
    MouseMode,
    /// Opens the global "what can I say / press" overlay.
    WhatCanISay,
}

/// The one list of tool words. A new tool word is one line here.
pub const TOOL_WORDS: [(&str, ToolWord); 3] = [
    ("keyboard", ToolWord::Keyboard),
    ("mouse mode", ToolWord::MouseMode),
    (WHAT_CAN_I_SAY, ToolWord::WhatCanISay),
];

/// The tool a whole transcript names, if any.
#[must_use]
pub fn tool_word(text: &str) -> Option<ToolWord> {
    let normalized = tehuti_protocol::normalize_phrase(text);
    TOOL_WORDS
        .iter()
        .find_map(|(word, tool)| (*word == normalized).then_some(*tool))
}
