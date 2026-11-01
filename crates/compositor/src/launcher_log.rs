//! One log line per launcher use, and the report read back from those lines.
use core::time::Duration;
use tehuti_core::launcher::{Ending, Report, Usage};

const MARKER: &str = "launcher use ";

#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    query: String,
    candidates: Vec<String>,
    /// Rank from 1, or absent when the use did not choose.
    chosen: Option<usize>,
    ending: String,
    ms_since_transcript: u64,
}

#[must_use]
pub fn line(usage: &Usage) -> String {
    let (chosen, ending) = match usage.ending {
        Ending::Chosen { rank } => (Some(rank), "chosen"),
        Ending::Cancelled => (None, "cancelled"),
        Ending::Replaced => (None, "replaced"),
    };
    let line = Line {
        query: usage.query.clone(),
        candidates: usage.candidates.clone(),
        chosen,
        ending: ending.into(),
        ms_since_transcript: u64::try_from(usage.since_transcript.as_millis()).unwrap_or(u64::MAX),
    };
    let json = serde_json::to_string(&line).unwrap_or_default();
    format!("{MARKER}{json}")
}

/// Reads a use back from any log line that holds one, such as a journal line with a prefix.
#[must_use]
pub fn parse(text: &str) -> Option<Usage> {
    let start = text.find(MARKER)?.saturating_add(MARKER.len());
    let json = text.get(start..)?;
    let line: Line = serde_json::Deserializer::from_str(json)
        .into_iter()
        .next()?
        .ok()?;
    let ending = match (line.ending.as_str(), line.chosen) {
        ("chosen", Some(rank)) if rank >= 1 => Ending::Chosen { rank },
        ("cancelled", None) => Ending::Cancelled,
        ("replaced", None) => Ending::Replaced,
        _ => return None,
    };
    Some(Usage {
        query: line.query,
        candidates: line.candidates,
        ending,
        since_transcript: Duration::from_millis(line.ms_since_transcript),
    })
}

#[must_use]
pub fn render(report: &Report) -> String {
    let mut text = format!(
        "uses: {}\nchosen: {}\nrank 1 chosen: {}\ncancelled: {}\nreplaced by new speech: {}\n",
        report.uses, report.chosen, report.first_rank, report.cancelled, report.replaced
    );
    if !report.mishears.is_empty() {
        let rows = report
            .mishears
            .iter()
            .map(|(heard, chosen)| format!("  {heard:?} -> {chosen:?}\n"))
            .collect::<Vec<_>>()
            .concat();
        text.push_str("heard -> chosen:\n");
        text.push_str(&rows);
    }
    text
}
