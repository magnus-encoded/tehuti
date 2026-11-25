//! The keyboard report: reads the modality log and ranks the situations that still needed the keyboard.

use super::{KeyClass, Line, Modality, Record, SpeechOutcome, Surface};
use core::fmt::Write as _;
use std::collections::BTreeMap;

/// Keyboard presses of one class in one app closer together than this count as one occurrence.
pub const BURST_GAP_MS: u64 = 2_000;

/// One app and key class where the keyboard was used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Situation {
    pub app_id: Option<String>,
    pub key_class: KeyClass,
    /// Bursts of presses: what a person would call one use of the keyboard.
    pub occurrences: u64,
    pub presses: u64,
    /// Fallbacks: presses that came soon after an unmatched transcript.
    pub after_unmatched_speech: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Ranked: most occurrences first, then most fallbacks.
    pub situations: Vec<Situation>,
    pub inputs: BTreeMap<Modality, u64>,
    pub speech: BTreeMap<SpeechOutcome, u64>,
    pub surfaces: BTreeMap<(Surface, Modality), u64>,
    pub sessions: u64,
    pub records: u64,
    /// Lines that did not parse, such as a line cut off by a crash.
    pub skipped: u64,
}

/// Parses log text, skipping lines that do not parse.
#[must_use]
pub fn parse(text: &str) -> (Vec<Line>, u64) {
    let mut skipped = 0_u64;
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            serde_json::from_str::<Line>(line)
                .inspect_err(|_| skipped = skipped.saturating_add(1))
                .ok()
        })
        .collect();
    (lines, skipped)
}

type Key = (Option<String>, KeyClass);

#[derive(Default)]
struct Tally {
    occurrences: u64,
    presses: u64,
    fallbacks: u64,
}

/// Counts the lines, in file order, into a ranked report.
#[must_use]
pub fn build(lines: &[Line], skipped: u64) -> Report {
    let mut report = Report {
        skipped,
        ..Report::default()
    };
    let mut tallies: BTreeMap<Key, Tally> = BTreeMap::new();
    let mut last_press: Option<(Key, u64)> = None;
    for line in lines {
        report.records = report.records.saturating_add(1);
        match &line.record {
            Record::SessionStart => report.sessions = report.sessions.saturating_add(1),
            Record::SessionEnd => {}
            Record::Input {
                modality,
                app_id,
                key_class,
            } => {
                bump(report.inputs.entry(*modality).or_default());
                if let (Modality::Keyboard, Some(class)) = (modality, key_class) {
                    let key = (app_id.clone(), *class);
                    let continues = last_press.as_ref().is_some_and(|(last, when)| {
                        *last == key && line.time_unix_ms.saturating_sub(*when) <= BURST_GAP_MS
                    });
                    let tally = tallies.entry(key.clone()).or_default();
                    bump(&mut tally.presses);
                    if !continues {
                        bump(&mut tally.occurrences);
                    }
                    last_press = Some((key, line.time_unix_ms));
                }
            }
            Record::Speech { outcome, .. } => bump(report.speech.entry(*outcome).or_default()),
            Record::Fallback {
                app_id, key_class, ..
            } => bump(&mut tallies.entry((app_id.clone(), *key_class)).or_default().fallbacks),
            Record::SurfaceOpened {
                surface, modality, ..
            } => bump(report.surfaces.entry((*surface, *modality)).or_default()),
        }
    }
    report.situations = tallies
        .into_iter()
        .map(|((app_id, key_class), tally)| Situation {
            app_id,
            key_class,
            occurrences: tally.occurrences,
            presses: tally.presses,
            after_unmatched_speech: tally.fallbacks,
        })
        .collect();
    report.situations.sort_by(|left, right| {
        right
            .occurrences
            .cmp(&left.occurrences)
            .then(right.after_unmatched_speech.cmp(&left.after_unmatched_speech))
            .then_with(|| left.app_id.cmp(&right.app_id))
            .then(left.key_class.cmp(&right.key_class))
    });
    report
}

const fn bump(count: &mut u64) {
    *count = count.saturating_add(1);
}

const fn app_name(app_id: Option<&str>) -> &str {
    match app_id {
        Some(name) => name,
        None => "(desktop)",
    }
}

/// The report as text, with at most `top` situations.
#[must_use]
pub fn render(report: &Report, top: usize) -> String {
    let mut text = String::new();
    let _sessions = writeln!(
        text,
        "Top keyboard-requiring situations ({} sessions, {} records, {} skipped)",
        report.sessions, report.records, report.skipped
    );
    let _header = writeln!(
        text,
        "{:>4}  {:<24} {:<12} {:>6} {:>8} {:>16}",
        "rank", "app", "kind", "times", "presses", "after failed say"
    );
    for (rank, situation) in report.situations.iter().take(top).enumerate() {
        let _row = writeln!(
            text,
            "{:>4}  {:<24} {:<12} {:>6} {:>8} {:>16}",
            rank.saturating_add(1),
            app_name(situation.app_id.as_deref()),
            situation.key_class.as_str(),
            situation.occurrences,
            situation.presses,
            situation.after_unmatched_speech
        );
    }
    if report.situations.is_empty() {
        let _none = writeln!(text, "      no keyboard use recorded");
    }
    let inputs: Vec<String> = report
        .inputs
        .iter()
        .map(|(modality, count)| format!("{} {count}", modality.as_str()))
        .collect();
    let _inputs = writeln!(text, "Inputs: {}", or_none(&inputs));
    let speech: Vec<String> = report
        .speech
        .iter()
        .map(|(outcome, count)| format!("{} {count}", outcome_name(*outcome)))
        .collect();
    let _speech = writeln!(text, "Speech: {}", or_none(&speech));
    let surfaces: Vec<String> = report
        .surfaces
        .iter()
        .map(|((surface, modality), count)| format!("{} by {} {count}", surface.as_str(), modality.as_str()))
        .collect();
    let _surfaces = writeln!(text, "Opened: {}", or_none(&surfaces));
    text
}

fn or_none(parts: &[String]) -> String {
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

const fn outcome_name(outcome: SpeechOutcome) -> &'static str {
    match outcome {
        SpeechOutcome::Matched => "matched",
        SpeechOutcome::Unmatched => "unmatched",
        SpeechOutcome::Dictated => "dictated",
    }
}

/// Reads the rotated file and then the current one, oldest first; a missing file reads as empty.
///
/// # Errors
/// A file exists but could not be read.
pub fn read_log(path: &std::path::Path) -> std::io::Result<String> {
    let mut text = String::new();
    for file in [super::FileSink::rotated(path), path.to_path_buf()] {
        match std::fs::read_to_string(&file) {
            Ok(part) => text.push_str(&part),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(text)
}
