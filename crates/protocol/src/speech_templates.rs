//! Context-owned speech prefixes with a free tail.
use crate::{Action, MappingAction, SequenceStep, Text};

const TEMPLATES: [(&str, &str); 3] = [
    ("<name>", "{name}"),
    ("<project>", "{project}"),
    ("<number>", "{number}"),
];

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private helpers also satisfy unreachable_pub"
)]
pub(crate) fn is_template(phrase: &str) -> bool {
    TEMPLATES.iter().any(|(suffix, _)| phrase.ends_with(suffix))
}

fn action_has_slot(action: &MappingAction, slot: &str) -> bool {
    match action {
        MappingAction::Text { text } => text.as_str().contains(slot),
        MappingAction::Sequence { sequence } => sequence.steps().iter().any(|step| match step {
            SequenceStep::Text { text } => text.as_str().contains(slot),
            SequenceStep::Key { .. } => false,
        }),
        MappingAction::Key { .. }
        | MappingAction::Secret { .. }
        | MappingAction::Launch { .. }
        | MappingAction::Close { .. }
        | MappingAction::Restart { .. }
        | MappingAction::Notify { .. }
        | MappingAction::Layout { .. }
        | MappingAction::Ticker { .. }
        | MappingAction::UndoDictation { .. } => false,
    }
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private helpers also satisfy unreachable_pub"
)]
pub(crate) fn validate(phrase: &str, action: &MappingAction) -> Result<(), String> {
    for (suffix, slot) in TEMPLATES {
        if let Some(prefix) = phrase.strip_suffix(suffix) {
            if prefix.trim().is_empty() || !prefix.ends_with(char::is_whitespace) {
                return Err("speech template needs a prefix followed by a space".into());
            }
            if !action_has_slot(action, slot) {
                return Err(format!(
                    "speech template needs a text or sequence action containing {slot}"
                ));
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private helpers also satisfy unreachable_pub"
)]
pub(crate) fn expand(
    phrase: &str,
    action: &MappingAction,
    heard: &str,
    projects: &[(String, String)],
) -> Option<Vec<Action>> {
    for (suffix, slot) in TEMPLATES {
        let Some(prefix) = phrase.strip_suffix(suffix) else {
            continue;
        };
        let heard = heard.trim();
        if !heard.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
            continue;
        }
        let tail = heard
            .get(prefix.len()..)?
            .trim_end_matches(['.', '!', '?'])
            .trim();
        if tail.is_empty() {
            return None;
        }
        let value = match suffix {
            "<project>" => {
                let path = project(tail, projects)?;
                format!("'{}'", path.replace('\'', "'\\''"))
            }
            "<number>" => parse_number(tail)?.to_string(),
            _ => tail.to_owned(),
        };
        return substitute(action, slot, &value);
    }
    None
}

fn substitute(action: &MappingAction, slot: &str, value: &str) -> Option<Vec<Action>> {
    match action {
        MappingAction::Text { text } => {
            let expanded = Text::new(text.as_str().replace(slot, value)).ok()?;
            Some(vec![Action::Text { text: expanded }])
        }
        MappingAction::Sequence { sequence } => sequence
            .steps()
            .iter()
            .map(|step| match step {
                SequenceStep::Key { key, mods } => Some(Action::Key {
                    key: *key,
                    mods: mods.clone(),
                }),
                SequenceStep::Text { text } => {
                    let expanded = Text::new(text.as_str().replace(slot, value)).ok()?;
                    Some(Action::Text { text: expanded })
                }
            })
            .collect(),
        MappingAction::Key { .. }
        | MappingAction::Secret { .. }
        | MappingAction::Launch { .. }
        | MappingAction::Close { .. }
        | MappingAction::Restart { .. }
        | MappingAction::Notify { .. }
        | MappingAction::Layout { .. }
        | MappingAction::Ticker { .. }
        | MappingAction::UndoDictation { .. } => None,
    }
}

/// Parses a non-negative integer written as digits or English words (for `<number>` templates).
fn parse_number(tail: &str) -> Option<u64> {
    let trimmed = tail.trim();
    if !trimmed.is_empty() && trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return trimmed.parse::<u64>().ok();
    }
    let mut total: u64 = 0;
    let mut current: u64 = 0;
    let mut saw_number = false;
    for raw in trimmed.split(|character: char| character.is_whitespace() || character == '-') {
        if raw.is_empty() {
            continue;
        }
        let word = raw.to_ascii_lowercase();
        if word == "and" {
            continue;
        }
        if let Some(value) = small_number(&word) {
            current = current.checked_add(value)?;
            saw_number = true;
        } else if word == "hundred" {
            let base = if current == 0 { 1 } else { current };
            current = base.checked_mul(100)?;
            saw_number = true;
        } else if word == "thousand" {
            let base = if current == 0 { 1 } else { current };
            total = total.checked_add(base.checked_mul(1000)?)?;
            current = 0;
            saw_number = true;
        } else {
            return None;
        }
    }
    saw_number.then(|| total.checked_add(current)).flatten()
}

fn small_number(word: &str) -> Option<u64> {
    match word {
        "zero" => Some(0),
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        "eleven" => Some(11),
        "twelve" => Some(12),
        "thirteen" => Some(13),
        "fourteen" => Some(14),
        "fifteen" => Some(15),
        "sixteen" => Some(16),
        "seventeen" => Some(17),
        "eighteen" => Some(18),
        "nineteen" => Some(19),
        "twenty" => Some(20),
        "thirty" => Some(30),
        "forty" => Some(40),
        "fifty" => Some(50),
        "sixty" => Some(60),
        "seventy" => Some(70),
        "eighty" => Some(80),
        "ninety" => Some(90),
        _ => None,
    }
}

fn project<'a>(heard: &str, projects: &'a [(String, String)]) -> Option<&'a str> {
    let heard = crate::joined_key(heard);
    let mut scored: Vec<_> = projects
        .iter()
        .map(|(name, path)| (crate::similarity(&heard, &crate::joined_key(name)), path.as_str()))
        .collect();
    scored.sort_by_key(|(score, _)| core::cmp::Reverse(*score));
    let (score, path) = scored.first()?;
    let rival = scored.get(1).map_or(0, |(rival_score, _)| *rival_score);
    (*score >= 800 && rival.saturating_add(crate::MARGIN) <= *score).then_some(*path)
}
