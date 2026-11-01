//! Named adjustment entry points and controller ownership.

use super::update_sticks;
use super::{ButtonName, ButtonState, Effect, Event, State, Sticks};

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
        let phrase = tehuti_protocol::normalize_phrase(text.as_str());
        if phrase == "volume" {
            if state.adjuster.is_none() && state.volume_pending == crate::adjuster::VolumeRead::Idle {
                state.volume_pending = crate::adjuster::VolumeRead::Pending;
                effects.push(Effect::ReadVolume);
            }
            return true;
        }
        let mut matches = state.adjusters.iter().filter(|(_, spec)| spec.name() == phrase);
        if let Some((plugin, spec)) = matches.next()
            && matches.next().is_none()
            && state.adjuster.is_none()
            && state.volume_pending == crate::adjuster::VolumeRead::Idle
        {
            state.adjuster = Some(crate::adjuster::Gauge::new(
                spec.name().into(),
                spec.minimum(),
                spec.maximum(),
                spec.value(),
                Some(*plugin),
            ));
            seed_stick(state);
            return true;
        }
    }
    false
}

pub(super) fn adjuster_input(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if state.volume_pending == crate::adjuster::VolumeRead::Pending {
        if let Event::Button {
            name,
            state: ButtonState::Pressed,
        } = event
        {
            state.adjuster_buttons.push(*name);
            if matches!(name, ButtonName::South | ButtonName::East) {
                state.volume_pending = crate::adjuster::VolumeRead::Cancelled;
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
            if *name == ButtonName::East {
                gauge.revert();
                effects.push(adjuster_effect(gauge, true));
                state.adjuster = None;
            } else if *name == ButtonName::South {
                effects.push(adjuster_effect(gauge, true));
                remember_adjuster(state);
                state.adjuster = None;
            }
        }
        return true;
    }
    matches!(event, Event::Speech { .. })
}

pub(super) fn adjuster_effect(gauge: &crate::adjuster::Gauge, finished: bool) -> Effect {
    gauge.plugin.map_or_else(
        || Effect::SetVolume(u8::try_from(gauge.value()).unwrap_or_default()),
        |plugin| Effect::AdjusterValue {
            plugin,
            name: gauge.name.clone(),
            value: gauge.value(),
            finished,
        },
    )
}

fn remember_adjuster(state: &mut State) {
    if let Some(gauge) = &state.adjuster
        && let Some((_, spec)) = state
            .adjusters
            .iter_mut()
            .find(|(plugin, spec)| Some(*plugin) == gauge.plugin && spec.name() == gauge.name)
    {
        spec.set_value(gauge.value());
    }
}

/// True when `text` names the built-in volume adjuster or a registered one.
pub(super) fn is_adjuster_phrase(state: &State, text: &str) -> bool {
    let phrase = tehuti_protocol::normalize_phrase(text);
    phrase == "volume" || state.adjusters.iter().any(|(_, spec)| spec.name() == phrase)
}

/// Starts a freshly opened gauge from the stick already held.
pub(super) fn seed_stick(state: &mut State) {
    if let Some(gauge) = &mut state.adjuster {
        let _released = gauge.sticks(state.adjuster_stick);
    }
}

pub(super) fn adjuster_sticks(state: &mut State, sticks: Sticks, effects: &mut Vec<Effect>) {
    state.adjuster_stick = sticks.left.1;
    if state.volume_pending == crate::adjuster::VolumeRead::Pending {
        return;
    }
    if let Some(gauge) = &mut state.adjuster {
        if gauge.sticks(sticks.left.1) {
            effects.push(adjuster_effect(gauge, true));
            remember_adjuster(state);
            state.adjuster = None;
        }
    } else {
        update_sticks(state, sticks);
    }
}
