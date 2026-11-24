//! First-word verbs that verb plugins own (ADR 0007).
//!
//! A transcript whose first word sounds like a plugin's verb goes to that plugin,
//! in any window, with the verb stripped. Words fold with the same sound key as
//! fuzzy phrase matching, so "Invoce, ..." still reaches the owner of "invoke".

use tehuti_protocol::{MIN_FUZZY_LETTERS, Text, fold_phrase, similarity};

/// The least similarity, in thousandths, a heard first word needs to a verb.
pub const VERB_SCORE: u16 = 850;

/// A transcript that began with a plugin's verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbMatch {
    /// The owning plugin, as the caller numbered it.
    pub plugin: usize,
    /// The verb as the plugin spelled it.
    pub verb: String,
    /// The pinned decoding language, when the owner supplied one.
    pub locale: Option<String>,
    /// The words after the verb.
    pub text: Text,
}

fn key(word: &str) -> String {
    fold_phrase(word).replace(' ', "")
}

/// How well a heard first word matches a verb, if it does: equal sound keys always
/// match; keys of five letters or more also match fuzzily at [`VERB_SCORE`]. A word
/// that extends the verb, such as "literally" for "literal", is a different word.
fn score(heard: &str, verb: &str) -> Option<u16> {
    let verb = key(verb);
    if verb.is_empty() {
        return None;
    }
    if heard == verb {
        return Some(1000);
    }
    if heard.starts_with(&verb) {
        return None;
    }
    let long = heard.chars().count() >= MIN_FUZZY_LETTERS && verb.chars().count() >= MIN_FUZZY_LETTERS;
    let score = similarity(heard, &verb);
    (long && score >= VERB_SCORE).then_some(score)
}

/// A registered verb's spelling and optional decoding language.
pub trait VerbDefinition: AsRef<str> {
    fn locale(&self) -> Option<&str>;
}

impl VerbDefinition for String {
    fn locale(&self) -> Option<&str> {
        None
    }
}

impl VerbDefinition for tehuti_protocol::Verb {
    fn locale(&self) -> Option<&str> {
        match self {
            Self::Word(_) => None,
            Self::Pinned(verb) => Some(&verb.locale),
        }
    }
}

/// Routes a transcript to the verb its first word matches best.
///
/// `owners` lists each plugin's verbs in priority order; on equal scores the earlier
/// owner wins. A lone verb with nothing after it matches nothing.
#[must_use]
pub fn route<'verbs, V: VerbDefinition + 'verbs>(
    owners: impl IntoIterator<Item = (usize, &'verbs [V])>,
    transcript: &str,
) -> Option<VerbMatch> {
    let trimmed = transcript.trim_start();
    let (first, rest) = trimmed.split_once(char::is_whitespace)?;
    let remainder = rest.trim_start_matches(|character: char| {
        character.is_whitespace() || matches!(character, ',' | '.' | ':' | ';' | '!' | '?')
    });
    if remainder.trim().is_empty() {
        return None;
    }
    let heard = key(first);
    if heard.is_empty() {
        return None;
    }
    let mut best: Option<(u16, usize, &V)> = None;
    for (plugin, verbs) in owners {
        for verb in verbs {
            if let Some(found) = score(&heard, verb.as_ref())
                && best.is_none_or(|(leader, _, _)| found > leader)
            {
                best = Some((found, plugin, verb));
            }
        }
    }
    let (_, plugin, verb) = best?;
    Some(VerbMatch {
        plugin,
        verb: verb.as_ref().into(),
        locale: verb.locale().map(str::to_owned),
        text: Text::new(remainder.into()).ok()?,
    })
}
