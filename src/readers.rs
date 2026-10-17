// SPDX-License-Identifier: MPL-2.0
use crate::spec::{Move, ValueType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Environment {
    pub programs: Vec<String>,
    /// Paths relative to the working folder, supplied by the host.
    pub names: Vec<String>,
    /// Folders relative to the working folder, supplied by the host.
    #[serde(default)]
    pub folders: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub confidence: Confidence,
    pub note: String,
}

#[must_use]
pub fn strip_transcript(text: &str) -> &str {
    text.trim().trim_end_matches(['.', '!', '?', ','])
}

#[must_use]
pub fn spelled(text: &str) -> String {
    let words = strip_transcript(text).replace("question mark", "question");
    let mut result = String::new();
    for word in words.split_whitespace() {
        let lower = word.to_lowercase();
        let part = match lower.as_str() {
            "star" | "asterisk" => "*",
            "dot" | "period" | "point" => ".",
            "slash" => "/",
            "dash" | "hyphen" | "minus" => "-",
            "underscore" => "_",
            "question" => "?",
            "tilde" => "~",
            "space" => " ",
            "pipe" => "|",
            "plus" => "+",
            "caret" => "^",
            "dollar" => "$",
            "backslash" => "\\",
            "comma" => ",",
            "colon" => ":",
            _ => word,
        };
        if part.len() > 1
            && part.chars().all(char::is_alphanumeric)
            && result.chars().next_back().is_some_and(char::is_alphanumeric)
        {
            result.push(' ');
        }
        result.push_str(part);
    }
    result
}

pub(crate) fn similarity(heard: &str, target: &str) -> usize {
    let heard = heard.to_lowercase();
    let target = target.to_lowercase();
    if heard == target {
        return 100;
    }
    if target.contains(&heard) || heard.contains(&target) {
        return 75;
    }
    let mut previous: Vec<usize> = (0..=target.chars().count()).collect();
    for (row, left) in heard.chars().enumerate() {
        let mut current = vec![row.saturating_add(1)];
        for (column, right) in target.chars().enumerate() {
            let above = previous
                .get(column.saturating_add(1))
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            let diagonal = previous
                .get(column)
                .copied()
                .unwrap_or(0)
                .saturating_add(usize::from(left != right));
            let before = current.last().copied().unwrap_or(0).saturating_add(1);
            current.push(above.min(diagonal).min(before));
        }
        previous = current;
    }
    let length = heard.chars().count().max(target.chars().count()).max(1);
    100_usize.saturating_sub(
        previous
            .last()
            .copied()
            .unwrap_or(length)
            .saturating_mul(100)
            .checked_div(length)
            .unwrap_or(100),
    )
}

const fn confidence(score: usize) -> Confidence {
    match score {
        90.. => Confidence::High,
        65..=89 => Confidence::Medium,
        _ => Confidence::Low,
    }
}

pub(crate) fn ranked(mut values: Vec<(String, usize, String)>) -> Vec<Candidate> {
    values.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let mut result: Vec<Candidate> = Vec::new();
    for (value, score, note) in values {
        if !result.iter().any(|candidate| candidate.value == value) {
            result.push(Candidate {
                value,
                confidence: confidence(score),
                note,
            });
        }
    }
    result.truncate(8);
    result
}

/// Read a transcript, or browse with `None`, using host-supplied facts only.
#[must_use]
pub fn read_value(definition: &Move, transcript: Option<&str>, env: &Environment) -> Vec<Candidate> {
    let literal = transcript.map(strip_transcript);
    let said = transcript.map(spelled);
    let values = match definition.takes {
        Some(ValueType::Path) => path_values(transcript, said.as_deref(), &env.names),
        Some(ValueType::Folder) => folder_values(transcript, said.as_deref(), env),
        Some(ValueType::Enum) => definition
            .values
            .iter()
            .flat_map(|values| values.iter())
            .map(|(value, description)| {
                (
                    value.clone(),
                    literal.map_or(100, |heard| {
                        similarity(heard, value).max(similarity(heard, description))
                    }),
                    description.clone(),
                )
            })
            .collect(),
        Some(ValueType::Integer) => integer_values(literal),
        Some(ValueType::Command) => env
            .programs
            .iter()
            .map(|program| {
                (
                    program.clone(),
                    literal.map_or(100, |heard| similarity(heard, program)),
                    "program".into(),
                )
            })
            .collect(),
        Some(ValueType::Glob | ValueType::Regex | ValueType::String) => {
            let (Some(literal), Some(said)) = (literal, said) else {
                return Vec::new();
            };
            let mut values = vec![
                (said.clone(), 95_usize, String::new()),
                (literal.into(), 85, "as heard".into()),
                (said.to_lowercase(), 90, String::new()),
            ];
            if definition.takes == Some(ValueType::Glob) {
                for (value, score, note) in &mut values {
                    let count = glob::Pattern::new(value).map_or(0, |pattern| {
                        env.names
                            .iter()
                            .filter(|name| pattern.matches(name.rsplit('/').next().unwrap_or(name)))
                            .count()
                    });
                    *note = format!("{count} matches");
                    if count == 0 {
                        *score = score.saturating_sub(30);
                    }
                }
            }
            values
        }
        None => Vec::new(),
    };
    ranked(values)
}

/// Parse keyboard shell words without evaluating shell expansions.
///
/// # Errors
/// Returns an error for unmatched shell quotes or NUL bytes.
pub fn parse_words(text: &str) -> Result<Vec<String>, String> {
    if text.contains('\0') {
        return Err("NUL cannot occur in an argument".into());
    }
    shell_words::split(text).map_err(|error| error.to_string())
}

/// Decode spoken symbols while preserving argument boundaries.
#[must_use]
pub fn spoken_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut tokens = strip_transcript(text).split_whitespace().peekable();
    while let Some(word) = tokens.next() {
        if words.is_empty() && !is_symbol(word) {
            words.push(word.into());
            continue;
        }
        if matches!(word.to_lowercase().as_str(), "dash" | "hyphen" | "minus") {
            let option = tokens.next().unwrap_or("");
            words.push(format!("-{}", option.to_lowercase()));
            continue;
        }
        let mut parts = vec![word.to_owned()];
        while let Some(next) = tokens.peek() {
            let lower = next.to_lowercase();
            let last = parts.last().map_or("", String::as_str);
            if matches!(lower.as_str(), "dash" | "hyphen" | "minus") {
                break;
            }
            if is_symbol(&lower)
                || matches!(last.to_lowercase().as_str(), "slash" | "dot" | "period" | "point")
                || next.len() == 1 && parts.iter().any(|part| is_symbol(part))
            {
                parts.push((*next).to_owned());
                let _consumed = tokens.next();
            } else {
                break;
            }
        }
        words.push(if parts.len() > 1 || is_symbol(word) {
            spelled(&parts.join(" "))
        } else {
            word.into()
        });
    }
    words
}

fn is_symbol(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "star"
            | "asterisk"
            | "dot"
            | "period"
            | "point"
            | "slash"
            | "underscore"
            | "question"
            | "tilde"
            | "plus"
            | "caret"
            | "dollar"
            | "backslash"
    )
}

fn folder_values(
    transcript: Option<&str>,
    said: Option<&str>,
    env: &Environment,
) -> Vec<(String, usize, String)> {
    let mut folders = vec!["..".to_owned()];
    folders.extend(env.folders.iter().cloned());
    path_values(transcript, said, &folders)
}

fn path_values(
    transcript: Option<&str>,
    said: Option<&str>,
    names: &[String],
) -> Vec<(String, usize, String)> {
    let mut values: Vec<_> = core::iter::once(".")
        .chain(names.iter().map(String::as_str))
        .filter(|name| transcript.is_some() || !name.contains('/'))
        .map(|name| {
            (
                name.to_owned(),
                said.map_or(100, |heard| {
                    similarity(heard, name).max(similarity(heard, name.rsplit('/').next().unwrap_or(name)))
                }),
                "exists".into(),
            )
        })
        .collect();
    if let Some(heard) = said {
        let exists = heard == "." || names.iter().any(|name| name == heard);
        values.push((
            heard.to_owned(),
            if exists { 100 } else { 50 },
            if exists { "exists" } else { "does not exist" }.into(),
        ));
    }
    values
}

fn integer_values(literal: Option<&str>) -> Vec<(String, usize, String)> {
    let Some(heard) = literal else {
        return Vec::new();
    };
    let lower = heard.to_lowercase();
    let number = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ]
    .iter()
    .position(|word| *word == lower)
    .map(|number| number.to_string());
    let value = number.unwrap_or_else(|| lower.clone());
    if value.parse::<i64>().is_err() {
        return Vec::new();
    }
    vec![(value, 100, String::new())]
}
