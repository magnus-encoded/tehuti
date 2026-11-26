//! Named adjustment entry points and controller ownership.

use super::update_sticks;
use super::{ButtonName, ButtonState, Effect, Event, State, Sticks};
use crate::adjuster::{
    BRIGHTNESS_MIN, BrightnessPhrase, Gauge, PendingRead, SizeAxis, brightness_unit_name, clamp_brightness,
    parse_brightness_phrase,
};

fn brightness_phrase(state: &mut State, text: &str, effects: &mut Vec<Effect>) -> bool {
    let Some(command) = parse_brightness_phrase(text) else {
        return false;
    };
    match command {
        BrightnessPhrase::Open => {
            if state.adjuster.is_none() && !state.pending_read.is_pending() {
                state.pending_read = PendingRead::Brightness;
                state.core.clear_bar_waiting();
                effects.push(Effect::ReadBrightness);
            }
        }
        BrightnessPhrase::Set(level) => {
            if let Some(gauge) = &mut state.adjuster {
                if gauge.is_brightness() {
                    let _changed = gauge.set_value(i32::from(level));
                    effects.push(Effect::SetBrightness(level));
                }
            } else if !state.pending_read.is_pending() {
                state.core.bar_result(&brightness_unit_name(level));
                effects.push(Effect::SetBrightness(level));
            }
        }
    }
    true
}

fn open_named_adjuster(state: &mut State, phrase: &str, effects: &mut Vec<Effect>) -> bool {
    if phrase == "volume" {
        if state.adjuster.is_none() && !state.pending_read.is_pending() {
            state.pending_read = PendingRead::Volume;
            state.core.clear_bar_waiting();
            effects.push(Effect::ReadVolume);
        }
        return true;
    }
    for axis in [SizeAxis::Width, SizeAxis::Height] {
        if phrase == axis.phrase() {
            if state.adjuster.is_none() && !state.pending_read.is_pending() {
                state.pending_read = PendingRead::Size(axis);
                state.core.clear_bar_waiting();
                effects.push(Effect::ReadWindowSize(axis));
            }
            return true;
        }
    }
    let mut matches = state.adjusters.iter().filter(|(_, spec)| spec.name() == phrase);
    if let Some((plugin, spec)) = matches.next()
        && matches.next().is_none()
        && state.adjuster.is_none()
        && !state.pending_read.is_pending()
    {
        state.core.clear_bar_waiting();
        state.adjuster = Some(Gauge::plugin(
            spec.name().into(),
            spec.minimum(),
            spec.maximum(),
            spec.value(),
            *plugin,
        ));
        seed_stick(state);
        return true;
    }
    false
}

pub(super) fn adjuster_phrase(
    state: &mut State,
    event: &Event,
    dictation: bool,
    effects: &mut Vec<Effect>,
) -> bool {
    if !dictation
        && let Event::Speech { text } = event
        && is_adjuster_phrase(state, text.as_str())
        && state
            .launcher
            .as_ref()
            .is_some_and(|launcher| launcher.list().is_open())
    {
        // An adjuster phrase opens its gauge rather than searching the launcher.
        state.launcher = None;
    }
    if !dictation
        && !state.keyboard.password_field_active()
        && !state.settings.open
        && !state.choice_list_open()
        && state.core.menu().is_none()
        && let Event::Speech { text } = event
    {
        if brightness_phrase(state, text.as_str(), effects) {
            return true;
        }
        let phrase = tehuti_protocol::normalize_phrase(text.as_str());
        return open_named_adjuster(state, &phrase, effects);
    }
    false
}

pub(super) fn adjuster_input(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    let family = state.core.pad_family();
    let confirm = family.button(crate::pad::Role::Confirm);
    let back = family.button(crate::pad::Role::Back);
    if state.pending_read.is_pending() {
        if let Event::Button {
            name,
            state: ButtonState::Pressed,
        } = event
        {
            state.adjuster_buttons.push(*name);
            if *name == confirm || *name == back {
                state.pending_read = PendingRead::Idle;
            }
        }
        return matches!(event, Event::Button { .. } | Event::Speech { .. });
    }
    let Some(gauge) = &mut state.adjuster else {
        return false;
    };
    if let Event::Button { name, state: button } = event {
        if *button == ButtonState::Pressed {
            state.adjuster_buttons.push(*name);
            let delta = if matches!(name, ButtonName::DpadRight | ButtonName::DpadUp) {
                1
            } else if matches!(name, ButtonName::DpadLeft | ButtonName::DpadDown) {
                -1
            } else {
                0
            };
            if delta != 0 {
                if gauge.press_step(*name, delta) {
                    effects.push(adjuster_effect(gauge, false));
                }
            } else if *name == back {
                gauge.revert();
                effects.push(adjuster_effect(gauge, true));
                state.adjuster = None;
            } else if *name == confirm {
                let label = gauge.is_brightness().then(|| gauge.unit_label());
                effects.push(adjuster_effect(gauge, true));
                remember_adjuster(state);
                state.adjuster = None;
                if let Some(label) = label {
                    state.core.bar_result(&label);
                }
            }
        } else {
            gauge.release_button(*name);
        }
        return true;
    }
    matches!(event, Event::Speech { .. })
}

pub(super) fn adjuster_effect(gauge: &Gauge, finished: bool) -> Effect {
    if let Some((axis, millionths)) = gauge.fraction() {
        return Effect::SetWindowSize { axis, millionths };
    }
    if gauge.is_brightness() {
        let level = u8::try_from(gauge.value()).unwrap_or(BRIGHTNESS_MIN);
        return Effect::SetBrightness(clamp_brightness(level));
    }
    gauge.plugin_id().map_or_else(
        || Effect::SetVolume(u8::try_from(gauge.value()).unwrap_or_default()),
        |plugin| Effect::AdjusterValue {
            plugin,
            name: gauge.name.clone(),
            value: gauge.value(),
            finished,
        },
    )
}

pub(super) fn remember_adjuster(state: &mut State) {
    if let Some(gauge) = &state.adjuster
        && let Some((_, spec)) = state
            .adjusters
            .iter_mut()
            .find(|(plugin, spec)| Some(*plugin) == gauge.plugin_id() && spec.name() == gauge.name)
    {
        spec.set_value(gauge.value());
    }
}

/// True when `text` names a built-in adjuster or a registered one.
pub(super) fn is_adjuster_phrase(state: &State, text: &str) -> bool {
    if parse_brightness_phrase(text).is_some() {
        return true;
    }
    let phrase = tehuti_protocol::normalize_phrase(text);
    phrase == "volume"
        || phrase == SizeAxis::Width.phrase()
        || phrase == SizeAxis::Height.phrase()
        || state.adjusters.iter().any(|(_, spec)| spec.name() == phrase)
}

/// Starts a freshly opened gauge from the stick already held.
pub(super) fn seed_stick(state: &mut State) {
    if let Some(gauge) = &mut state.adjuster {
        gauge.sticks(state.adjuster_stick);
    }
}

pub(super) fn adjuster_sticks(state: &mut State, sticks: Sticks) {
    state.adjuster_stick = sticks.left.1;
    if state.pending_read.is_pending() {
        return;
    }
    if let Some(gauge) = &mut state.adjuster {
        gauge.sticks(sticks.left.1);
    } else {
        update_sticks(state, sticks);
    }
}
