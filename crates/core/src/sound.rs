//! Built-in sound commands and output matching (ADR 0008).
//!
//! Voice names destinations ("mute", "unmute", "microphone mute", "microphone on",
//! "speaker <name>"); the controller steps relatively (toggle mute, toggle microphone
//! mute, cycle audio output).

use tehuti_protocol::{ButtonName, MARGIN, fold_phrase, normalize_phrase, similarity};

/// Minimum sound-key similarity, in thousandths, for matching a spoken output name.
const OUTPUT_MATCH_SCORE: u16 = 800;

/// One audio output (sink) reported by the audio backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioOutput {
    /// Backend node identifier passed to `wpctl set-default`.
    pub id: u32,
    /// Human-readable description shown by the audio server.
    pub name: String,
    /// Whether this output is currently the default sink.
    pub default: bool,
}

impl AudioOutput {
    /// The normalized output name voice uses after "speaker".
    #[must_use]
    pub fn spoken_name(&self) -> String {
        normalize_phrase(&self.name)
    }

    /// The full phrase voice uses to select this output directly.
    #[must_use]
    pub fn voice_phrase(&self) -> String {
        format!("speaker {}", self.spoken_name())
    }

    /// The bar signpost shown when this output becomes active.
    #[must_use]
    pub fn signpost(&self) -> String {
        format!("Speaker {}", self.spoken_name())
    }
}

/// A spoken sound destination: every variant names an end state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundCommand {
    /// Set speaker mute to an explicit state ("mute" or "unmute").
    SetMute(bool),
    /// Set microphone mute to an explicit state ("microphone mute" or "microphone on").
    SetMicrophoneMute(bool),
    /// Switch to the named audio output ("speaker <query>").
    SelectOutput(String),
}

impl SoundCommand {
    /// Parses a whole transcript naming a built-in sound destination.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let normalized = normalize_phrase(text);
        match normalized.as_str() {
            "mute" => Some(Self::SetMute(true)),
            "unmute" => Some(Self::SetMute(false)),
            "microphone mute" => Some(Self::SetMicrophoneMute(true)),
            "microphone on" => Some(Self::SetMicrophoneMute(false)),
            _ => normalized
                .strip_prefix("speaker ")
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(|query| Self::SelectOutput(query.into())),
        }
    }

    /// The bar text for speaker mute state; matches what voice says for it.
    #[must_use]
    pub const fn mute_signpost(muted: bool) -> &'static str {
        if muted { "Mute" } else { "Unmute" }
    }

    /// The bar text for microphone mute state; matches what voice says for it.
    #[must_use]
    pub const fn microphone_signpost(muted: bool) -> &'static str {
        if muted { "Microphone mute" } else { "Microphone on" }
    }
}

/// Whether `phrase` names a built-in sound destination owned by core.
#[must_use]
pub fn is_reserved_phrase(phrase: &str) -> bool {
    SoundCommand::parse(phrase).is_some()
}

/// Relative sound steps triggered by Guide chords on the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundStep {
    /// Toggle speaker mute (`Guide + West`).
    ToggleMute,
    /// Toggle microphone mute (`Guide + North`).
    ToggleMicrophoneMute,
    /// Cycle to the next audio output (`Guide + East`).
    NextOutput,
}

impl SoundStep {
    /// The sound step a button performs when pressed with Guide held, if any.
    #[must_use]
    pub const fn from_chord_button(button: ButtonName) -> Option<Self> {
        match button {
            ButtonName::West => Some(Self::ToggleMute),
            ButtonName::North => Some(Self::ToggleMicrophoneMute),
            ButtonName::East => Some(Self::NextOutput),
            ButtonName::South
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::LeftTrigger
            | ButtonName::RightTrigger
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick
            | ButtonName::DpadUp
            | ButtonName::DpadDown
            | ButtonName::DpadLeft
            | ButtonName::DpadRight
            | ButtonName::Paddle1
            | ButtonName::Paddle2
            | ButtonName::Paddle3
            | ButtonName::Paddle4 => None,
        }
    }
}

/// The next audio output in cycle order after the current default.
#[must_use]
pub fn next_output(outputs: &[AudioOutput]) -> Option<&AudioOutput> {
    if outputs.is_empty() {
        return None;
    }
    let next = outputs
        .iter()
        .position(|output| output.default)
        .and_then(|current| current.saturating_add(1).checked_rem(outputs.len()))
        .unwrap_or(0);
    outputs.get(next)
}

/// Matches a spoken output query against known audio outputs by name and sound.
#[must_use]
pub fn match_output<'outputs>(
    outputs: &'outputs [AudioOutput],
    query: &str,
) -> Option<&'outputs AudioOutput> {
    let normalized = normalize_phrase(query);
    if normalized.is_empty() {
        return None;
    }
    if let Some(exact) = outputs.iter().find(|output| output.spoken_name() == normalized) {
        return Some(exact);
    }
    let query_words: Vec<&str> = normalized.split_whitespace().collect();
    if let Some(unique) = unique_word_match(outputs, &normalized, &query_words) {
        return Some(unique);
    }
    match_by_sound(outputs, &normalized, query_words.len() == 1)
}

fn unique_word_match<'outputs>(
    outputs: &'outputs [AudioOutput],
    normalized: &str,
    query_words: &[&str],
) -> Option<&'outputs AudioOutput> {
    let mut matches = outputs.iter().filter(|output| {
        let spoken = output.spoken_name();
        let words: Vec<&str> = spoken.split_whitespace().collect();
        spoken.starts_with(normalized)
            || query_words
                .iter()
                .all(|wanted| words.iter().any(|word| word == wanted))
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn match_by_sound<'outputs>(
    outputs: &'outputs [AudioOutput],
    normalized: &str,
    single_word: bool,
) -> Option<&'outputs AudioOutput> {
    let folded_query = fold_phrase(normalized);
    if folded_query.is_empty() {
        return None;
    }
    let mut scored: Vec<(u16, &AudioOutput)> = outputs
        .iter()
        .filter_map(|output| {
            let spoken = output.spoken_name();
            let full = similarity(&folded_query, &fold_phrase(&spoken));
            let by_word = if single_word {
                spoken
                    .split_whitespace()
                    .map(|word| similarity(&folded_query, &fold_phrase(word)))
                    .max()
                    .unwrap_or_default()
            } else {
                0
            };
            let score = full.max(by_word);
            (score >= OUTPUT_MATCH_SCORE).then_some((score, output))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0));
    let (best_score, best_output) = scored.first().copied()?;
    let clear_winner = scored
        .get(1)
        .is_none_or(|(runner_up, _)| best_score >= runner_up.saturating_add(MARGIN));
    clear_winner.then_some(best_output)
}
