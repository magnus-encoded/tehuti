//! Built-in tool words: spoken in any context, matched before plugin routing.

/// A built-in tool a spoken word opens or toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolWord {
    /// Opens the on-screen keyboard.
    Keyboard,
    /// Toggles pointer mode.
    MouseMode,
}

/// The one list of tool words. A new tool word is one line here.
pub const TOOL_WORDS: [(&str, ToolWord); 2] = [
    ("keyboard", ToolWord::Keyboard),
    ("mouse mode", ToolWord::MouseMode),
];

/// The tool a whole transcript names, if any.
#[must_use]
pub fn tool_word(text: &str) -> Option<ToolWord> {
    let normalized = tehuti_protocol::normalize_phrase(text);
    TOOL_WORDS
        .iter()
        .find_map(|(word, tool)| (*word == normalized).then_some(*tool))
}
