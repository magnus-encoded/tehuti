//! The pure core step: state and one event in, state and actions out.

use tehuti_protocol::{Action, ButtonName, ButtonState, Context, Event, KeyName};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    contexts: Vec<Context>,
    focused_app_id: Option<String>,
    guide_held: Option<core::time::Duration>,
    menu: Option<menu::Menu>,
    commands: Vec<menu::Command>,
    pending: Vec<plugin::Pending>,
    bar: bar::Bar,
}

impl State {
    #[must_use]
    pub const fn bar(&self) -> &bar::Bar {
        &self.bar
    }

    pub fn capture(
        &mut self,
        state: speech::SpeechState,
        input: speech::SpeechInput,
    ) -> (speech::SpeechState, Vec<speech::SpeechEffect>) {
        let fresh_press = !state.held;
        let observation = input.clone();
        let (next, effects) = speech::speech_step(state, input);
        self.bar.capture(&observation, &effects, fresh_press);
        (next, effects)
    }

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
            menu: None,
            commands: Vec::new(),
            pending: Vec::new(),
            bar: bar::Bar::new(),
        }
    }

    /// Swaps the contexts and keeps focus, held buttons and pending plugin decisions.
    pub fn replace_contexts(&mut self, contexts: Vec<Context>) {
        self.contexts = contexts;
    }

    /// The installed contexts, in priority order.
    #[must_use]
    pub fn contexts(&self) -> &[Context] {
        &self.contexts
    }

    /// Cancels controller holds and closes the system menu when the device disconnects or the seat pauses.
    pub const fn controller_disconnected(&mut self) {
        self.guide_held = None;
        self.menu = None;
    }

    /// The system menu while it is open.
    #[must_use]
    pub const fn menu(&self) -> Option<&menu::Menu> {
        self.menu.as_ref()
    }

    /// Takes the system commands the menu chose since the last call.
    pub fn take_commands(&mut self) -> Vec<menu::Command> {
        core::mem::take(&mut self.commands)
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
    if !matches!(event, Event::Focus { .. }) {
        state.bar.event(event);
    }
    let (mut next, actions, routed) = route(state, event, plugin_active);
    if matches!(event, Event::Focus { .. }) {
        next.bar.event(event);
    }
    if matches!(event, Event::Speech { .. }) && !routed {
        next.bar.resolve(&actions);
    }
    (next, actions, routed)
}

fn route(mut state: State, event: &Event, plugin_active: bool) -> (State, Vec<Action>, bool) {
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
    if state.menu.is_some() {
        let outcome = menu::handle(&mut state.menu, event);
        state.commands.extend(outcome.commands);
        return (state, outcome.actions, false);
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
        if text.as_str().trim().is_empty() {
            return None;
        }
        return focused_context(state)
            .and_then(|context| context.speech_action(text.as_str()))
            .or_else(|| global_context(state).and_then(|context| context.speech_action(text.as_str())))
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
    focused_context(state)
        .and_then(|context| context.action_for(*name))
        .or_else(|| global_context(state).and_then(|context| context.action_for(*name)))
}

/// App id of the global context, whose mappings apply wherever the focused context has none.
pub const GLOBAL_APP_ID: &str = "*";

fn focused_context(state: &State) -> Option<&Context> {
    state
        .contexts
        .iter()
        .find(|context| Some(context.app_id()) == state.focused_app_id.as_deref())
}

fn global_context(state: &State) -> Option<&Context> {
    state
        .contexts
        .iter()
        .find(|context| context.app_id() == GLOBAL_APP_ID)
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
                    state.menu = Some(menu::Menu::default());
                }
            }
            Some(Vec::new())
        }
        Event::Button { .. } | Event::Focus { .. } | Event::Speech { .. } => None,
    }
}

pub mod menu;
pub mod secret;
pub mod speech;

pub mod plugin;

pub mod dispatch;

pub mod controller;

pub mod windows;

pub mod layout;

pub mod bar;
