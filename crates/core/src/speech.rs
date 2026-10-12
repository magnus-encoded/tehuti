//! Deterministic capture lifecycle and transcript admission.

use tehuti_protocol::Text;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpeechState {
    available: bool,
    held: bool,
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
    Transcript { session: u64, text: Text },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeechEffect {
    Start { session: u64 },
    Stop { session: u64, transcribe: bool },
    Speech { session: u64, text: Text },
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
        SpeechInput::Transcript { session, text } => {
            if state.pending == Some(session) {
                state.pending = None;
                effects.push(SpeechEffect::Speech { session, text });
            }
        }
    }
    (state, effects)
}
