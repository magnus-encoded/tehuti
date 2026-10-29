//! Voice feedback driven by admitted capture events, routing outcomes and elapsed time.

use core::time::Duration;
use tehuti_protocol::{Action, Event};

use crate::speech::{SpeechEffect, SpeechFailure, SpeechInput};

pub const HEIGHT: i32 = 32;
pub const RESULT_LIFETIME: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Context,
    Unfocused,
    Live,
    Result,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub tone: Tone,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    context: Option<String>,
    focus: Option<(Option<String>, String)>,
    labels: Vec<(String, String)>,
    agents: Vec<String>,
    windows: Vec<(String, bool)>,
    feedback: Option<Segment>,
    remaining: Option<Duration>,
    level: crate::Level,
    pub(crate) generation: u64,
    ticker: Ticker,
}

/// The mapping ticker's entries, whether it is hidden, and how long it has scrolled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Ticker {
    entries: Option<std::sync::Arc<[crate::ticker::Entry]>>,
    hidden: bool,
    scrolled: Duration,
}

/// What the ticker shows now: its entries and the scroll step reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TickerView {
    pub entries: std::sync::Arc<[crate::ticker::Entry]>,
    pub step: u64,
}

/// The chip text for a focused window.
///
/// A window titled by its agent launcher
/// (`agent`, or `agent: <project>`) shows the agent or the project; a window
/// whose context sets `label` shows the label; every other window shows its app id.
#[must_use]
pub fn context_label(
    app_id: Option<&str>,
    title: &str,
    label: Option<&str>,
    agents: &[String],
) -> Option<String> {
    for agent in agents
        .iter()
        .map(String::as_str)
        .filter(|agent| !agent.is_empty())
    {
        if title == agent {
            return Some(agent.into());
        }
        if let Some(project) = title
            .strip_prefix(agent)
            .and_then(|rest| rest.strip_prefix(": "))
            .map(str::trim)
            .filter(|project| !project.is_empty())
        {
            return Some(project.into());
        }
    }
    label.or(app_id).map(Into::into)
}

/// The short chip text for a window at columns level.
#[must_use]
pub fn window_label(
    label: Option<&str>,
    title: Option<&str>,
    app_id: Option<&str>,
    agents: &[String],
) -> String {
    context_label(app_id, title.unwrap_or_default(), label, agents).unwrap_or_else(|| "Untitled".into())
}

impl Bar {
    /// The short name of a window using the configured agent launchers.
    #[must_use]
    pub fn window_label(&self, label: Option<&str>, title: Option<&str>, app_id: Option<&str>) -> String {
        window_label(label, title, app_id, &self.agents)
    }

    #[must_use]
    pub const fn new() -> Self {
        Self {
            context: None,
            focus: None,
            labels: Vec::new(),
            agents: Vec::new(),
            windows: Vec::new(),
            feedback: None,
            remaining: None,
            level: crate::Level::App,
            generation: 0,
            ticker: Ticker {
                entries: None,
                hidden: false,
                scrolled: Duration::ZERO,
            },
        }
    }

    /// The ticker to draw, or `None` while it is disabled, hidden or empty.
    #[must_use]
    pub fn ticker(&self) -> Option<TickerView> {
        if self.ticker.hidden {
            return None;
        }
        let entries = self
            .ticker
            .entries
            .clone()
            .filter(|entries| !entries.is_empty())?;
        let step = self
            .ticker
            .scrolled
            .as_millis()
            .checked_div(crate::ticker::STEP.as_millis())
            .and_then(|step| u64::try_from(step).ok())
            .unwrap_or_default();
        Some(TickerView { entries, step })
    }

    /// Replaces the ticker entries, or disables it with `None`; new entries scroll from the start.
    pub(crate) fn set_ticker(&mut self, entries: Option<Vec<crate::ticker::Entry>>) {
        let entries = entries.map(std::sync::Arc::from);
        if entries != self.ticker.entries {
            self.ticker.entries = entries;
            self.ticker.scrolled = Duration::ZERO;
        }
    }

    pub(crate) const fn toggle_ticker(&mut self) {
        self.ticker.hidden = !self.ticker.hidden;
    }

    pub(crate) fn set_agents(&mut self, agents: Vec<String>) {
        self.agents = agents;
    }

    /// Sets the per-application chip labels and relabels the focused window.
    pub(crate) fn set_labels(&mut self, labels: Vec<(String, String)>) {
        if labels != self.labels {
            self.labels = labels;
            if let Some((app_id, title)) = &self.focus {
                self.context = context_label(
                    app_id.as_deref(),
                    title,
                    self.label(app_id.as_deref()),
                    &self.agents,
                );
            }
        }
    }

    fn label(&self, app_id: Option<&str>) -> Option<&str> {
        let app_id = app_id?;
        self.labels
            .iter()
            .find(|(labelled, _)| labelled == app_id)
            .map(|(_, label)| label.as_str())
    }

    #[must_use]
    pub const fn level(&self) -> crate::Level {
        self.level
    }

    #[must_use]
    pub fn segments(&self) -> Vec<Segment> {
        let mut segments: Vec<Segment> = if self.level == crate::Level::Columns && !self.windows.is_empty() {
            self.windows
                .iter()
                .map(|(title, focused)| Segment {
                    text: title.clone(),
                    tone: if *focused { Tone::Context } else { Tone::Unfocused },
                })
                .collect()
        } else {
            vec![Segment {
                text: self.context.clone().unwrap_or_else(|| "Desktop".into()),
                tone: Tone::Context,
            }]
        };
        if self.level == crate::Level::App
            && let Some((_, title)) = &self.focus
            && !title.is_empty()
            && self.context.as_deref() != Some(title.as_str())
        {
            segments.push(Segment {
                text: title.clone(),
                tone: Tone::Unfocused,
            });
        }
        segments.extend(self.feedback.clone());
        segments
    }

    /// The segment belonging to the focused window, independent of feedback tones.
    #[must_use]
    pub fn focused_window_segment(&self) -> Option<usize> {
        self.windows.iter().position(|(_, focused)| *focused)
    }

    #[must_use]
    pub fn with_windows(mut self, windows: Vec<(String, bool)>) -> Self {
        self.windows = windows;
        self
    }

    /// Adds feedback to a bar view without changing its window focus.
    #[must_use]
    pub fn with_feedback(mut self, feedback: Segment) -> Self {
        self.feedback = Some(feedback);
        self
    }

    pub(crate) const fn set_level(&mut self, level: crate::Level) {
        self.level = level;
    }

    pub(crate) fn event(&mut self, event: &Event) {
        match event {
            Event::Focus { app_id, title } => {
                self.generation = self.generation.saturating_add(1);
                self.context = context_label(
                    app_id.as_deref(),
                    title,
                    self.label(app_id.as_deref()),
                    &self.agents,
                );
                self.focus = Some((app_id.clone(), title.clone()));
                self.feedback = None;
                self.remaining = None;
            }
            Event::Tick { elapsed } => {
                if self.ticker.entries.is_some() && !self.ticker.hidden {
                    self.ticker.scrolled = self.ticker.scrolled.saturating_add(*elapsed);
                }
                if let Some(remaining) = &mut self.remaining {
                    *remaining = remaining.saturating_sub(*elapsed);
                    if remaining.is_zero() {
                        self.feedback = None;
                        self.remaining = None;
                    }
                }
            }
            Event::Speech { text } => {
                self.generation = self.generation.saturating_add(1);
                if text.as_str().trim().is_empty() {
                    self.result("No speech", Tone::Error);
                } else {
                    self.live("Resolving…");
                }
            }
            Event::Button { .. } => {}
        }
    }

    pub(crate) fn capture(&mut self, input: &SpeechInput, effects: &[SpeechEffect], fresh_press: bool) {
        match input {
            SpeechInput::Cancel | SpeechInput::Disconnected => {
                self.generation = self.generation.saturating_add(1);
                self.feedback = None;
                self.remaining = None;
            }
            SpeechInput::Unavailable => {
                self.generation = self.generation.saturating_add(1);
            }
            SpeechInput::Press if fresh_press && effects.is_empty() => {
                self.result("Speech unavailable", Tone::Error);
            }
            SpeechInput::Press
            | SpeechInput::Release
            | SpeechInput::Available
            | SpeechInput::Interim { .. }
            | SpeechInput::Transcript { .. }
            | SpeechInput::Failed { .. } => {}
        }
        for effect in effects {
            match effect {
                SpeechEffect::Start { .. } => {
                    self.generation = self.generation.saturating_add(1);
                    self.live("Listening…");
                }
                SpeechEffect::Interim { text, .. } => self.live(text.as_str()),
                SpeechEffect::Stop { transcribe: true, .. } => self.live("Transcribing…"),
                SpeechEffect::Stop {
                    transcribe: false, ..
                } => {
                    self.feedback = None;
                    self.remaining = None;
                }
                SpeechEffect::Speech { .. } => {}
                SpeechEffect::Failed { reason, .. } => self.result(
                    match reason {
                        SpeechFailure::Silent => "Mic silent",
                        SpeechFailure::Transcription => "Speech failed",
                    },
                    Tone::Error,
                ),
            }
        }
        if *input == SpeechInput::Unavailable {
            self.result("Speech unavailable", Tone::Error);
        }
    }

    fn live(&mut self, text: &str) {
        self.feedback = Some(Segment {
            text: text.into(),
            tone: Tone::Live,
        });
        self.remaining = None;
    }

    pub(crate) fn keyboard_feedback(&mut self, text: &str) {
        self.generation = self.generation.saturating_add(1);
        self.result(text, Tone::Result);
    }

    fn result(&mut self, text: &str, tone: Tone) {
        self.feedback = Some(Segment {
            text: text.into(),
            tone,
        });
        self.remaining = Some(RESULT_LIFETIME);
    }

    pub(crate) fn resolve(&mut self, actions: &[Action]) {
        for action in actions {
            let (text, tone) = match action {
                Action::Text { .. } => ("typed".into(), Tone::Result),
                Action::Key { key, .. } | Action::KeyDown { key } => (format!("{key:?}"), Tone::Result),
                Action::KeyUp { .. } => continue,
                Action::Launch { desktop_entry } => (desktop_entry.as_str().into(), Tone::Result),
                Action::Notice { text } => {
                    if text.as_str() == "NO SPEECH MAPPING" {
                        ("No speech mapping".into(), Tone::Error)
                    } else {
                        (text.as_str().into(), Tone::Result)
                    }
                }
                Action::SecretRequest { .. } => ("Confirm secret".into(), Tone::Result),
                Action::Close => ("Closed".into(), Tone::Result),
                Action::Layout { .. } => ("Layout".into(), Tone::Result),
                Action::ToggleTicker => ("Ticker".into(), Tone::Result),
                Action::Quit => ("Quitting".into(), Tone::Result),
            };
            self.result(&text, tone);
        }
    }
}
