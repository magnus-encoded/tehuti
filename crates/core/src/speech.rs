//! Deterministic capture lifecycle and transcript admission.

use tehuti_protocol::Text;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpeechState {
    available: bool,
    pub(crate) held: bool,
    next_session: u64,
    recording: Option<u64>,
    pending: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeechInput {
    Press,
    Release,
    Cancel,
    Disconnected,
    Unavailable,
    Available,
    Interim { session: u64, text: Text },
    Transcript { session: u64, text: Text },
    Failed { session: u64, reason: SpeechFailure },
}

/// Why a released session produced no transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeechFailure {
    /// The captured audio was empty or digital silence.
    Silent,
    /// Transcription itself failed.
    Transcription,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeechEffect {
    Start { session: u64 },
    Stop { session: u64, transcribe: bool },
    Interim { session: u64, text: Text },
    Speech { session: u64, text: Text },
    Failed { session: u64, reason: SpeechFailure },
}

#[must_use]
pub fn speech_step(mut state: SpeechState, input: SpeechInput) -> (SpeechState, Vec<SpeechEffect>) {
    let mut effects = Vec::new();
    match input {
        SpeechInput::Press => {
            if !state.held {
                state.held = true;
                state.pending = None;
                if state.available
                    && let Some(next) = state.next_session.checked_add(1)
                {
                    let session = state.next_session;
                    state.next_session = next;
                    state.recording = Some(session);
                    effects.push(SpeechEffect::Start { session });
                }
            }
        }
        SpeechInput::Release => {
            state.held = false;
            if let Some(session) = state.recording.take() {
                state.pending = Some(session);
                effects.push(SpeechEffect::Stop {
                    session,
                    transcribe: true,
                });
            }
        }
        SpeechInput::Cancel | SpeechInput::Disconnected | SpeechInput::Unavailable => {
            if input == SpeechInput::Unavailable {
                state.available = false;
            }
            if input == SpeechInput::Disconnected {
                state.held = false;
            }
            state.pending = None;
            if let Some(session) = state.recording.take() {
                effects.push(SpeechEffect::Stop {
                    session,
                    transcribe: false,
                });
            }
        }
        SpeechInput::Available => state.available = true,
        SpeechInput::Interim { session, text } => {
            if state.recording == Some(session) {
                effects.push(SpeechEffect::Interim { session, text });
            }
        }
        SpeechInput::Transcript { .. } | SpeechInput::Failed { .. } => {
            if let Some(effect) = final_effect(&mut state, input) {
                effects.push(effect);
            }
        }
    }
    (state, effects)
}

fn final_effect(state: &mut SpeechState, input: SpeechInput) -> Option<SpeechEffect> {
    let (session, effect) = match input {
        SpeechInput::Transcript { session, text } => (session, SpeechEffect::Speech { session, text }),
        SpeechInput::Failed { session, reason } => (session, SpeechEffect::Failed { session, reason }),
        SpeechInput::Press
        | SpeechInput::Release
        | SpeechInput::Cancel
        | SpeechInput::Disconnected
        | SpeechInput::Unavailable
        | SpeechInput::Available
        | SpeechInput::Interim { .. } => return None,
    };
    if state.pending != Some(session) {
        return None;
    }
    state.pending = None;
    Some(effect)
}
