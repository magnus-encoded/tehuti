//! Opt-in fuzzy speech matching for phrases that miss an exact mapping.
//!
//! Whisper spells Norwegian loosely ("Åpne Pokeman Rød", "Lukke vinduet"), so a
//! phrase that matches nothing exactly is folded into a rough sound key and
//! compared by edit distance with every mapped phrase. A match needs a unique
//! winner above the context's threshold, and destructive actions need more.

use crate::{Action, normalize_phrase};

/// Folded keys shorter than this, without spaces, never match fuzzily: "Ned" or "Hva" would hit anything.
pub const MIN_FUZZY_LETTERS: usize = 5;

/// The least score, in thousandths, a destructive action needs, whatever the threshold.
pub const DESTRUCTIVE_SCORE: u16 = 900;

/// How far the winner must lead the best phrase with another action, in thousandths.
pub const MARGIN: u16 = 50;

/// Launcher entries that leave the session or open power controls.
const DESTRUCTIVE_LAUNCHES: [&str; 7] = [
    "to-xfce", "power", "shutdown", "reboot", "logout", "suspend", "menu",
];

/// A similarity threshold in thousandths, from `fuzzy = 0.8`; zero turns fuzzy matching off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(into = "f32")]
pub struct Fuzziness(u16);

impl Fuzziness {
    pub const OFF: Self = Self(0);

    /// A threshold in thousandths, capped at 1000.
    #[must_use]
    pub fn from_thousandths(thousandths: u16) -> Self {
        Self(thousandths.min(1000))
    }

    #[must_use]
    pub const fn thousandths(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn is_off(self) -> bool {
        self.0 == 0
    }
}

impl From<Fuzziness> for f32 {
    #[expect(
        clippy::float_arithmetic,
        reason = "thousandths become the configured fraction"
    )]
    fn from(value: Fuzziness) -> Self {
        Self::from(value.0) / 1000.0
    }
}

impl<'de> serde::Deserialize<'de> for Fuzziness {
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::float_arithmetic,
        reason = "finite unit interval is rounded into bounded thousandths"
    )]
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Setting {
            Enabled(bool),
            Threshold(f32),
        }
        let value = match Setting::deserialize(deserializer)? {
            Setting::Enabled(true) => 0.8,
            Setting::Enabled(false) => 0.0,
            Setting::Threshold(value) => value,
        };
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(serde::de::Error::custom("fuzzy must be between 0 and 1"));
        }
        Ok(Self((value * 1000.0).round() as u16))
    }
}

/// Speech matching options: `[speech_matching]` with `fuzzy = true` or a numeric threshold.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechMatching {
    pub fuzzy: Option<Fuzziness>,
}

/// A phrase a transcript matched only fuzzily, for routing and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// The mapped phrase as the context file spells it.
    pub phrase: String,
    pub action: Action,
    /// Similarity in thousandths; 1000 means the folded keys are equal.
    pub score: u16,
}

/// A mapped phrase ready for fuzzy comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate<'context> {
    pub phrase: &'context str,
    pub folded: &'context str,
    pub action: Action,
}

/// Whether an action closes, quits or leaves the session, so a loose match must not fire it.
#[must_use]
pub fn destructive(action: &Action) -> bool {
    match action {
        Action::Close | Action::Quit => true,
        Action::Launch { desktop_entry } => DESTRUCTIVE_LAUNCHES
            .iter()
            .any(|part| desktop_entry.as_str().contains(part)),
        Action::Key { .. }
        | Action::KeyDown { .. }
        | Action::KeyUp { .. }
        | Action::Text { .. }
        | Action::SecretRequest { .. }
        | Action::Notice { .. }
        | Action::Layout { .. }
        | Action::ToggleTicker => false,
    }
}

/// Spellings folded together, applied in order to each word.
const SPELLINGS: [(&str, &str); 11] = [
    ("æ", "e"),
    ("ø", "o"),
    ("ae", "e"),
    ("oe", "o"),
    ("ph", "f"),
    ("th", "t"),
    ("ch", "k"),
    ("c", "k"),
    ("q", "k"),
    ("w", "v"),
    ("z", "s"),
];

/// Word starts Norwegian does not sound: "hva" is said "va", "gjøre" "jøre".
const SILENT_STARTS: [(&str, &str); 3] = [("hv", "v"), ("hj", "j"), ("gj", "j")];

/// Inflection endings stripped from words of four letters or more, longest first.
const ENDINGS: [&str; 6] = ["ene", "et", "en", "er", "e", "t"];

/// A rough Norwegian sound key for fuzzy matching.
///
/// The phrase is normalized, accents dropped, æ ø å folded to e o a,
/// spelling variants and doubled letters merged, and inflection endings stripped,
/// so "Åpner Pokémon Gull" and "Åpne Pokemon Gul" fold to the same key.
#[must_use]
pub fn fold_phrase(phrase: &str) -> String {
    fold_words(phrase, true).join(" ")
}

/// The key of a phrase with its words run together and no endings stripped, so that
/// "Super. Tux." and `SuperTux`. both give `supertuks`.
#[must_use]
pub fn joined_key(phrase: &str) -> String {
    fold_words(phrase, false).concat()
}

fn fold_words(phrase: &str, strip: bool) -> Vec<String> {
    use unicode_normalization::{UnicodeNormalization as _, char::is_combining_mark};
    normalize_phrase(phrase)
        .split(' ')
        .map(|word| {
            let mut word: String = word
                .nfd()
                .filter(|character| !is_combining_mark(*character) && *character != '\'')
                .collect();
            for (written, folded) in SPELLINGS {
                word = word.replace(written, folded);
            }
            for (written, spoken) in SILENT_STARTS {
                if let Some(rest) = word.strip_prefix(written) {
                    word = format!("{spoken}{rest}");
                }
            }
            word = word.replace('x', "ks");
            let mut collapsed = String::with_capacity(word.len());
            for character in word.chars() {
                if !collapsed.ends_with(character) {
                    collapsed.push(character);
                }
            }
            if strip {
                strip_ending(&collapsed)
            } else {
                collapsed
            }
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn strip_ending(word: &str) -> String {
    if word.chars().count() >= 4 {
        for ending in ENDINGS {
            if let Some(stem) = word.strip_suffix(ending)
                && stem.chars().count() >= 3
            {
                return stem.into();
            }
        }
    }
    word.into()
}

/// The optimal string alignment distance: insertions, deletions, substitutions and
/// swaps of neighbouring characters each cost one.
#[must_use]
pub fn damerau_levenshtein(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut table = vec![vec![0usize; right.len().saturating_add(1)]; left.len().saturating_add(1)];
    let cell = |cells: &[Vec<usize>], row: usize, column: usize| {
        cells
            .get(row)
            .and_then(|line| line.get(column))
            .copied()
            .unwrap_or_default()
    };
    for (row, cells) in table.iter_mut().enumerate() {
        for (column, value) in cells.iter_mut().enumerate() {
            *value = if row == 0 {
                column
            } else if column == 0 {
                row
            } else {
                0
            };
        }
    }
    for (row, left_char) in left.iter().enumerate() {
        for (column, right_char) in right.iter().enumerate() {
            let (below, after) = (row.saturating_add(1), column.saturating_add(1));
            let mut best = (cell(&table, row, column).saturating_add(usize::from(left_char != right_char)))
                .min(cell(&table, row, after).saturating_add(1))
                .min(cell(&table, below, column).saturating_add(1));
            if row > 0
                && column > 0
                && left.get(row.saturating_sub(1)) == Some(right_char)
                && right.get(column.saturating_sub(1)) == Some(left_char)
            {
                best =
                    best.min(cell(&table, row.saturating_sub(1), column.saturating_sub(1)).saturating_add(1));
            }
            if let Some(value) = table.get_mut(below).and_then(|cells| cells.get_mut(after)) {
                *value = best;
            }
        }
    }
    cell(&table, left.len(), right.len())
}

/// Similarity of two folded keys in thousandths: 1000 less the edit distance per character.
#[must_use]
pub fn similarity(left: &str, right: &str) -> u16 {
    let longest = left.chars().count().max(right.chars().count());
    if longest == 0 {
        return 1000;
    }
    let distance = damerau_levenshtein(left, right).min(longest);
    let lost = distance
        .saturating_mul(1000)
        .checked_div(longest)
        .unwrap_or_default();
    u16::try_from(1000usize.saturating_sub(lost)).unwrap_or_default()
}

/// Whether each part of a heard key is close to the same part of a mapped key.
///
/// With two or more words on both sides, the first word, such as "åpne", and the
/// rest, such as the app name, are compared on their own, so a shared "Åpne" cannot
/// carry a different name. A part passes at `threshold`, or with a single edit,
/// since one letter weighs heavily in a short word.
#[must_use]
pub fn parts_agree(heard: &str, mapped: &str, threshold: Fuzziness) -> bool {
    let close = |left: &str, right: &str| {
        similarity(left, right) >= threshold.thousandths() || damerau_levenshtein(left, right) <= 1
    };
    match (heard.split_once(' '), mapped.split_once(' ')) {
        (Some((heard_first, heard_rest)), Some((mapped_first, mapped_rest))) => {
            close(heard_first, mapped_first) && close(heard_rest, mapped_rest)
        }
        _ => true,
    }
}

/// The similarity of a heard key to a mapped key, or `None` when their parts disagree.
///
/// Whisper splits and joins words differently from the phrase (`Super Tux` for `SuperTux`),
/// so the keys are also compared with every space removed. The better score wins. With two
/// words or more on both sides the first word must still be close, so a shared "Åpne"
/// cannot carry a different name.
#[must_use]
pub fn key_score(heard: &str, mapped: &str, joined: (&str, &str), threshold: Fuzziness) -> Option<u16> {
    let plain = parts_agree(heard, mapped, threshold).then(|| similarity(heard, mapped));
    let first_agrees = match (heard.split_once(' '), mapped.split_once(' ')) {
        (Some((heard_first, _)), Some((mapped_first, _))) => {
            similarity(heard_first, mapped_first) >= threshold.thousandths()
                || damerau_levenshtein(heard_first, mapped_first) <= 1
        }
        _ => true,
    };
    let joined = first_agrees.then(|| similarity(joined.0, joined.1));
    plain
        .into_iter()
        .chain(joined)
        .chain(window_score(heard, mapped, threshold))
        .max()
}

/// The most a bare name found inside a longer sentence ("Hun er supertux") may score, in thousandths.
const WINDOW_CAP: u16 = 900;

/// A one-word mapped key found as one word or two joined words inside a longer heard key.
fn window_score(heard: &str, mapped: &str, threshold: Fuzziness) -> Option<u16> {
    if mapped.contains(' ') || !heard.contains(' ') {
        return None;
    }
    let words: Vec<&str> = heard.split(' ').collect();
    let single = words.iter().map(|word| (*word).to_owned());
    let pairs = words.windows(2).map(<[&str]>::concat);
    single
        .chain(pairs)
        .map(|window| similarity(&window, mapped))
        .max()
        .filter(|score| *score >= threshold.thousandths())
        .map(|score| score.min(WINDOW_CAP))
}

fn letters(folded: &str) -> usize {
    folded.chars().filter(|character| *character != ' ').count()
}

/// The unique mapped phrase `heard` is close to, if any.
///
/// Candidates come in priority order. A candidate qualifies when its [`similarity`]
/// (or that of the keys with spaces removed) reaches `threshold` and its parts agree ([`key_score`]); a destructive one also
/// needs [`DESTRUCTIVE_SCORE`]. The best qualifier wins
/// only if every phrase with another action whose parts agree scores at least
/// [`MARGIN`] lower.
/// Keys under [`MIN_FUZZY_LETTERS`] letters never match.
#[must_use]
pub fn best_match<'context>(
    heard: &str,
    candidates: impl IntoIterator<Item = Candidate<'context>>,
    threshold: Fuzziness,
) -> Option<FuzzyMatch> {
    if threshold.is_off() {
        return None;
    }
    let heard_joined = joined_key(heard);
    let heard = fold_phrase(heard);
    if letters(&heard) < MIN_FUZZY_LETTERS {
        return None;
    }
    let scored: Vec<(u16, Candidate<'context>)> = candidates
        .into_iter()
        .filter(|candidate| letters(candidate.folded) >= MIN_FUZZY_LETTERS)
        .filter_map(|candidate| {
            key_score(
                &heard,
                candidate.folded,
                (&heard_joined, &joined_key(candidate.phrase)),
                threshold,
            )
            .map(|score| (score, candidate))
        })
        .collect();
    let (best_score, best) = scored
        .iter()
        .filter(|(score, candidate)| {
            *score >= threshold.thousandths()
                && (!destructive(&candidate.action) || *score >= DESTRUCTIVE_SCORE)
        })
        .fold(None::<&(u16, Candidate<'context>)>, |best, scored| match best {
            Some(kept) if kept.0 >= scored.0 => Some(kept),
            _ => Some(scored),
        })?;
    let rival = scored
        .iter()
        .filter(|(_, candidate)| candidate.action != best.action)
        .map(|(score, _)| *score)
        .max()
        .unwrap_or_default();
    (rival.saturating_add(MARGIN) <= *best_score).then(|| FuzzyMatch {
        phrase: best.phrase.into(),
        action: best.action.clone(),
        score: *best_score,
    })
}
