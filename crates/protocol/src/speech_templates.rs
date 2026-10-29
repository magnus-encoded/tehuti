//! Context-owned speech prefixes with a free tail.
use crate::{Action, MappingAction, Text};

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private helpers also satisfy unreachable_pub"
)]
pub(crate) fn validate(phrase: &str, action: &MappingAction) -> Result<(), String> {
    for (suffix, slot) in [("<name>", "{name}"), ("<project>", "{project}")] {
        if let Some(prefix) = phrase.strip_suffix(suffix) {
            if prefix.trim().is_empty() || !prefix.ends_with(char::is_whitespace) {
                return Err("speech template needs a prefix followed by a space".into());
            }
            if !matches!(action, MappingAction::Text { text } if text.as_str().contains(slot)) {
                return Err(format!("speech template needs a text action containing {slot}"));
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
) -> Option<Action> {
    let MappingAction::Text { text } = action else {
        return None;
    };
    for (suffix, slot) in [("<name>", "{name}"), ("<project>", "{project}")] {
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
        let value = if suffix == "<project>" {
            let path = project(tail, projects)?;
            format!("'{}'", path.replace('\'', "'\\''"))
        } else {
            tail.to_owned()
        };
        return Text::new(text.as_str().replace(slot, &value))
            .ok()
            .map(|text| Action::Text { text });
    }
    None
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
