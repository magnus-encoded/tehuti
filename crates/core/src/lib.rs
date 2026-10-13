//! The pure core step: state and one event in, state and actions out.

use tehuti_protocol::{Action, ButtonName, ButtonState, Context, Event, KeyName};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    contexts: Vec<Context>,
    focused_app_id: Option<String>,
    guide_held: Option<core::time::Duration>,
    pending: Vec<plugin::Pending>,
}

impl State {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn with_contexts(contexts: Vec<Context>) -> Self {
        Self {
            contexts,
            focused_app_id: None,
            guide_held: None,
            pending: Vec::new(),
        }
    }

    /// Plugin decisions still awaiting a reply, expiry or plugin exit.
    #[must_use]
    pub const fn pending_decisions(&self) -> usize {
        self.pending.len()
    }
}

/// Advances the core by one event. Performs no I/O.
#[must_use]
pub fn step(state: State, event: &Event) -> (State, Vec<Action>) {
    let (next, actions, _) = step_with_plugin(state, event, false);
    (next, actions)
}

/// Routes to a plugin in place of the active context after higher layers consume input.
#[must_use]
pub fn step_with_plugin(mut state: State, event: &Event, plugin_active: bool) -> (State, Vec<Action>, bool) {
    if let Event::Tick { elapsed } = event {
        let mut actions = plugin::expire(&mut state, *elapsed);
        if let Some(mut chord_actions) = escape_chord(&mut state, event) {
            actions.append(&mut chord_actions);
        }
        return (state, actions, false);
    }
    if let Event::Focus { app_id, .. } = event {
        let actions = plugin::expire(&mut state, core::time::Duration::MAX);
        state.focused_app_id.clone_from(app_id);
        return (state, actions, false);
    }
    if let Some(actions) = escape_chord(&mut state, event) {
        return (state, actions, false);
    }
    if let Some(action) = global_layer(event) {
        return (state, vec![action], false);
    }
    if matches!(
        event,
        Event::Button {
            name: ButtonName::Record,
            ..
        }
    ) {
        return (state, Vec::new(), false);
    }
    if plugin_active && matches!(event, Event::Button { .. } | Event::Speech { .. }) {
        return (state, Vec::new(), true);
    }
    let actions = active_context(&state, event)
        .or_else(|| fallback(event))
        .into_iter()
        .collect();
    (state, actions, false)
}

/// The fixed global mapping takes priority over contexts.
const fn global_layer(event: &Event) -> Option<Action> {
    match *event {
        Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        } => Some(Action::Key { key: KeyName::Enter }),
        Event::Button { .. } | Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => None,
    }
}

fn active_context(state: &State, event: &Event) -> Option<Action> {
    if let Event::Speech { text } = event {
        return state
            .contexts
            .iter()
            .find(|context| Some(context.app_id()) == state.focused_app_id.as_deref())
            .and_then(|context| context.speech_action(text.as_str()))
            .or_else(|| {
                tehuti_protocol::Text::new("NO SPEECH MAPPING".into())
                    .ok()
                    .map(|notice| Action::Notice { text: notice })
            });
    }
    let Event::Button {
        name,
        state: ButtonState::Pressed,
    } = event
    else {
        return None;
    };
    state
        .contexts
        .iter()
        .find(|context| Some(context.app_id()) == state.focused_app_id.as_deref())
        .and_then(|context| context.action_for(*name))
}

const fn fallback(_event: &Event) -> Option<Action> {
    None
}

fn escape_chord(state: &mut State, event: &Event) -> Option<Vec<Action>> {
    match event {
        Event::Button {
            name: ButtonName::Guide,
            state: button_state,
        } => {
            match button_state {
                ButtonState::Pressed => {
                    let _held = state.guide_held.get_or_insert(core::time::Duration::ZERO);
                }
                ButtonState::Released => state.guide_held = None,
            }
            Some(Vec::new())
        }
        Event::Tick { elapsed } => {
            if let Some(held) = &mut state.guide_held {
                *held = held.saturating_add(*elapsed);
                if *held >= core::time::Duration::from_secs(3) {
                    state.guide_held = None;
                    return Some(vec![Action::Quit]);
                }
            }
            Some(Vec::new())
        }
        Event::Button { .. } | Event::Focus { .. } | Event::Speech { .. } => None,
    }
}

pub mod secret;
pub mod speech;

pub mod plugin;

pub mod controller;

pub mod windows;
