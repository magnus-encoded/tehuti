//! The built-in `invoke` verb: "invoke usage report" types `/usage-report`.
//!
//! The words after the verb snap to the closest slash-command word a context
//! knows; with no close word they are slugified as heard.

use tehuti_protocol::{MIN_FUZZY_LETTERS, fold_phrase, similarity};

/// The built-in verb word.
pub const VERB: &str = "invoke";

/// The least similarity, in thousandths, the spoken words need to a known word.
pub const SNAP_SCORE: u16 = 850;

fn key(words: &str) -> String {
    fold_phrase(&words.replace('-', " ")).replace(' ', "")
}

/// The spoken words as a slash-command name: lowercase letters and digits joined by hyphens.
#[must_use]
pub fn slugify(words: &str) -> String {
    words
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// The known word the spoken words sound like, if any. Equal sound keys always
/// match; keys of five letters or more also match at [`SNAP_SCORE`]. On equal
/// scores the earlier word wins.
#[must_use]
pub fn snap<'known>(words: &str, known: impl IntoIterator<Item = &'known str>) -> Option<&'known str> {
    let heard = key(words);
    if heard.is_empty() {
        return None;
    }
    let mut best: Option<(u16, &str)> = None;
    for word in known {
        let candidate = key(word);
        if candidate.is_empty() {
            continue;
        }
        let score = if candidate == heard {
            1000
        } else if heard.chars().count() >= MIN_FUZZY_LETTERS && candidate.chars().count() >= MIN_FUZZY_LETTERS
        {
            similarity(&heard, &candidate)
        } else {
            0
        };
        if score >= SNAP_SCORE && best.is_none_or(|(leader, _)| score > leader) {
            best = Some((score, word));
        }
    }
    best.map(|(_, word)| word)
}

/// The text the invoke verb types for a transcript, if it begins with the verb
/// and has words after it.
#[must_use]
pub fn slash_command<'known>(
    transcript: &str,
    known: impl IntoIterator<Item = &'known str>,
) -> Option<String> {
    let verbs = [VERB.to_owned()];
    let matched = crate::verbs::route([(0, verbs.as_slice())], transcript)?;
    let remainder = matched.text.as_str();
    let name = snap(remainder, known).map_or_else(|| slugify(remainder), str::to_owned);
    (!name.is_empty()).then(|| format!("/{name}"))
}
