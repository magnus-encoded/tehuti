//! Built-in sound speech routing and Guide chord steps.

use super::{ButtonState, Effect, Event, State};
use crate::ownership::{Owner, Why, owner};
use crate::sound::{AudioOutput, SoundCommand, SoundStep};

/// Handles a spoken sound destination ("mute", "unmute", "microphone mute", "microphone on", "speaker <name>").
pub(super) fn sound_phrase(
    state: &mut State,
    event: &Event,
    dictation: bool,
    effects: &mut Vec<Effect>,
) -> bool {
    let Event::Speech { text } = event else {
        return false;
    };
    let Some(command) = SoundCommand::parse(text.as_str()) else {
        return false;
    };
    if !dictation
        && state
            .launcher
            .as_ref()
            .is_some_and(|launcher| launcher.list().is_open())
    {
        state.launcher = None;
    }
    if dictation
        || state.keyboard.password_field_active()
        || state.settings.open
        || state.choice_list_open()
        || state.core.menu().is_some()
    {
        return false;
    }
    effects.push(match command {
        SoundCommand::SetMute(muted) => Effect::SetMute(muted),
        SoundCommand::SetMicrophoneMute(muted) => Effect::SetMicrophoneMute(muted),
        SoundCommand::SelectOutput(query) => Effect::SelectAudioOutput(query),
    });
    true
}

/// Handles a Guide sound chord press or release (`Guide + West/North/East`).
pub(super) fn sound_chord(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    let Event::Button { name, state: button } = event else {
        return false;
    };
    let mut view = state.core.ownership_view(false);
    view.passthrough = super::pad_passed_through(state);
    view.held_buttons.clone_from(&state.passthrough_buttons);
    if owner(&view, *name, (*button).into()) != Owner::Tehuti(Why::GuideSoundChord) {
        return false;
    }
    let fresh = *button == ButtonState::Pressed && !state.core.in_chord(*name);
    if let Some(actions) = crate::escape_chord(&mut state.core, event) {
        effects.extend(actions.into_iter().map(Effect::Apply));
    }
    if fresh && let Some(step) = SoundStep::from_chord_button(*name) {
        effects.push(match step {
            SoundStep::ToggleMute => Effect::ToggleMute,
            SoundStep::ToggleMicrophoneMute => Effect::ToggleMicrophoneMute,
            SoundStep::NextOutput => Effect::NextAudioOutput,
        });
    }
    true
}

/// Applies the audio worker's mute state reply to the bar.
pub(super) fn apply_mute(state: &mut State, muted: Option<bool>) {
    match muted {
        Some(muted) => state.core.bar_mut().set_muted(muted),
        None => state.core.bar_mut().sound_feedback("Sound unavailable", true),
    }
}

/// Applies the audio worker's microphone mute state reply to the bar.
pub(super) fn apply_microphone_mute(state: &mut State, muted: Option<bool>) {
    match muted {
        Some(muted) => state.core.bar_mut().set_mic_muted(muted),
        None => state.core.bar_mut().sound_feedback("Sound unavailable", true),
    }
}

/// Applies the audio worker's output selection reply to the bar.
pub(super) fn apply_output_selected(state: &mut State, result: Result<AudioOutput, String>) {
    match result {
        Ok(output) => state.core.bar_mut().sound_feedback(&output.signpost(), false),
        Err(reason) => state.core.bar_mut().sound_feedback(&reason, true),
    }
}
