//! Spoken key sequences are validated before any input is emitted.
use tehuti_protocol::{Action, KeyName, Text};

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private helpers also satisfy unreachable_pub"
)]
pub(crate) fn actions(phrase: &str) -> Option<Vec<Action>> {
    let normalized = tehuti_protocol::normalize_phrase(phrase);
    let tail = normalized.strip_prefix("type ")?;
    let words: Vec<_> = tail.split_whitespace().collect();
    let mut remaining = words.as_slice();
    let mut actions = Vec::new();
    while !remaining.is_empty() {
        let matched = (1..=3).rev().find_map(|count| {
            let name = remaining.get(..count)?.join(" ");
            key(&name).map(|action| (count, action))
        });
        let Some((count, action)) = matched else {
            return Text::new("UNKNOWN SPOKEN KEY".into())
                .ok()
                .map(|text| vec![Action::Notice { text }]);
        };
        actions.push(action);
        remaining = remaining.get(count..)?;
    }
    Some(actions)
}

fn symbol(phrase: &str) -> Option<&'static str> {
    match phrase {
        "slash" => Some("/"),
        "backslash" => Some("\\"),
        "dot" | "period" | "full stop" => Some("."),
        "comma" => Some(","),
        "colon" => Some(":"),
        "semicolon" => Some(";"),
        "quote" | "single quote" | "apostrophe" => Some("'"),
        "double quote" => Some("\""),
        "dash" | "minus" | "hyphen" => Some("-"),
        "underscore" => Some("_"),
        "plus" => Some("+"),
        "equals" | "equal" => Some("="),
        "asterisk" | "star" => Some("*"),
        "ampersand" => Some("&"),
        "pipe" | "vertical bar" => Some("|"),
        "tilde" => Some("~"),
        "backtick" | "grave" => Some("`"),
        "at" | "at sign" => Some("@"),
        "hash" | "pound" => Some("#"),
        "dollar" | "dollar sign" => Some("$"),
        "percent" => Some("%"),
        "caret" => Some("^"),
        "exclamation mark" => Some("!"),
        "question mark" => Some("?"),
        "left bracket" | "open bracket" => Some("["),
        "right bracket" | "close bracket" => Some("]"),
        "left brace" | "open brace" => Some("{"),
        "right brace" | "close brace" => Some("}"),
        "left parenthesis" | "open parenthesis" => Some("("),
        "right parenthesis" | "close parenthesis" => Some(")"),
        "less than" => Some("<"),
        "greater than" => Some(">"),
        _ => None,
    }
}

fn key(phrase: &str) -> Option<Action> {
    if let Some(symbol) = symbol(phrase) {
        return Text::new(symbol.into()).ok().map(|text| Action::Text { text });
    }
    let named = match phrase {
        "enter" | "return" => "Return",
        "escape" | "esc" => "Escape",
        "page up" => "Prior",
        "page down" => "Next",
        "zero" => "0",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        _ => phrase,
    };
    KeyName::ALL
        .iter()
        .find(|key| key.keysym_name().eq_ignore_ascii_case(named))
        .map(|key| Action::Key {
            key: *key,
            mods: Vec::new(),
        })
}
