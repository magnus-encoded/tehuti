//! Default voice phrases for layout actions (ADR 0008).
//!
//! Voice names whole units (a destination workspace, a layout state or a column move)
//! and never steps relatively.

use core::num::NonZeroU8;

use tehuti_protocol::LayoutAction;

const WORDS: [&str; 20] = [
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
];

fn workspace_number(word: &str) -> Option<NonZeroU8> {
    if let Some(index) = WORDS.iter().position(|known| *known == word) {
        return u8::try_from(index.saturating_add(1))
            .ok()
            .and_then(NonZeroU8::new);
    }
    if !word.is_empty() && !word.starts_with('0') && word.as_bytes().iter().all(u8::is_ascii_digit) {
        return word.parse::<u8>().ok().and_then(NonZeroU8::new);
    }
    None
}

/// Parses a default voice phrase that names a layout unit or destination.
///
/// Relative steps such as "next workspace" or "wider" never match; those belong to the pad.
#[must_use]
pub fn parse(phrase: &str) -> Option<LayoutAction> {
    let normalized = tehuti_protocol::normalize_phrase(phrase);
    match normalized.as_str() {
        "full screen" | "fullscreen" => return Some(LayoutAction::ToggleFullscreen),
        "full width" => return Some(LayoutAction::MaximizeColumn),
        "move left" => return Some(LayoutAction::MoveColumnLeft),
        "move right" => return Some(LayoutAction::MoveColumnRight),
        _ => {}
    }
    if let Some(rest) = normalized.strip_prefix("workspace ") {
        return workspace_number(rest).map(LayoutAction::FocusWorkspace);
    }
    if let Some(rest) = normalized.strip_prefix("send to workspace ") {
        if let Some(number) = rest.strip_suffix(" and stay") {
            return workspace_number(number).map(LayoutAction::MoveColumnToWorkspaceStay);
        }
        return workspace_number(rest).map(LayoutAction::MoveColumnToWorkspace);
    }
    if let Some(rest) = normalized.strip_prefix("send and stay ") {
        let rest = rest.strip_prefix("to ").unwrap_or(rest);
        let rest = rest.strip_prefix("workspace ").unwrap_or(rest);
        return workspace_number(rest).map(LayoutAction::MoveColumnToWorkspaceStay);
    }
    None
}
