//! Voice feedback driven by admitted capture events, routing outcomes and elapsed time.

use core::time::Duration;
use tehuti_protocol::{Action, Event};

use crate::speech::{SpeechEffect, SpeechFailure, SpeechInput};

pub const HEIGHT: i32 = 32;
pub const RESULT_LIFETIME: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Context,
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
    feedback: Option<Segment>,
    remaining: Option<Duration>,
    pub(crate) generation: u64,
}

impl Bar {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            context: None,
            feedback: None,
            remaining: None,
            generation: 0,
        }
    }

    #[must_use]
    pub fn segments(&self) -> Vec<Segment> {
        let mut segments = vec![Segment {
            text: self.context.clone().unwrap_or_else(|| "Desktop".into()),
            tone: Tone::Context,
        }];
        segments.extend(self.feedback.clone());
        segments
    }

    pub(crate) fn event(&mut self, event: &Event) {
        match event {
            Event::Focus { app_id, .. } => {
                self.generation = self.generation.saturating_add(1);
                self.context.clone_from(app_id);
                self.feedback = None;
                self.remaining = None;
            }
            Event::Tick { elapsed } => {
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
                Action::Key { key } => (format!("{key:?}"), Tone::Result),
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
                Action::Quit => ("Quitting".into(), Tone::Result),
            };
            self.result(&text, tone);
        }
    }
}
