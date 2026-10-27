//! The pure core step: state and one event in, state and actions out.

use tehuti_protocol::{Action, ButtonName, ButtonState, Context, Event, KeyName, LayoutAction};

/// The longest Guide press that still counts as a tap.
pub const GUIDE_TAP_WINDOW: core::time::Duration = core::time::Duration::from_millis(500);

/// What LB and RB drive; a Guide tap switches between the two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Level {
    /// LB and RB route to the focused app's context like any other button.
    #[default]
    App,
    /// LB and RB move focus one column left or right.
    Columns,
}

impl Level {
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::App => Self::Columns,
            Self::Columns => Self::App,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::App => "app level",
            Self::Columns => "columns level",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    contexts: Vec<Context>,
    focused_app_id: Option<String>,
    pad_taken_back: bool,
    guide_held: Option<core::time::Duration>,
    guide_tap: Option<core::time::Duration>,
    start_held: bool,
    level: Level,
    menu: Option<menu::Menu>,
    menu_buttons: Vec<ButtonName>,
    pending: Vec<plugin::Pending>,
    bar: bar::Bar,
    held_keys: Vec<(ButtonName, KeyName)>,
    pad_family: ticker::PadFamily,
}

impl State {
    #[must_use]
    pub fn passthrough_push_to_talk(&self) -> &[ButtonName] {
        focused_context(self)
            .or_else(|| global_context(self))
            .map_or(&[ButtonName::Record], Context::passthrough_push_to_talk)
    }

    #[must_use]
    pub fn passthrough(&self) -> bool {
        self.passthrough_context() && !self.pad_taken_back
    }

    pub(crate) fn passthrough_context(&self) -> bool {
        focused_context(self)
            .or_else(|| global_context(self))
            .is_some_and(Context::passthrough)
    }

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
    pub fn with_contexts(contexts: Vec<Context>) -> Self {
        let mut state = Self {
            contexts,
            focused_app_id: None,
            pad_taken_back: false,
            guide_held: None,
            guide_tap: None,
            start_held: false,
            level: Level::App,
            menu: None,
            menu_buttons: Vec::new(),
            pending: Vec::new(),
            bar: bar::Bar::new(),
            held_keys: Vec::new(),
            pad_family: ticker::PadFamily::Letters,
        };
        state.refresh_ticker();
        state
    }

    /// Resolves the focused app's dimming policy over the global context.
    #[must_use]
    pub fn dim_config(&self) -> (bool, f32, core::time::Duration) {
        let global = self
            .contexts
            .iter()
            .find(|context| context.app_id() == "*")
            .map(Context::dim);
        let active = self
            .contexts
            .iter()
            .find(|context| Some(context.app_id()) == self.focused_app_id.as_deref())
            .map(Context::dim);
        let enabled = active
            .and_then(|config| config.enabled)
            .or_else(|| global.and_then(|config| config.enabled))
            .unwrap_or(true);
        let strength = active
            .and_then(|config| config.strength)
            .or_else(|| global.and_then(|config| config.strength))
            .map_or(0.67, tehuti_protocol::DimStrength::multiplier);
        let fade = active
            .and_then(|config| config.fade_ms)
            .or_else(|| global.and_then(|config| config.fade_ms))
            .unwrap_or(100);
        (
            enabled,
            strength,
            core::time::Duration::from_millis(u64::from(fade)),
        )
    }

    /// Resolves per-application keyboard options over the `*` global context.
    #[must_use]
    pub fn keyboard_config(&self) -> (tehuti_protocol::OnTextField, tehuti_protocol::ButtonName) {
        let global = self.contexts.iter().find(|context| context.app_id() == "*");
        let active = self
            .contexts
            .iter()
            .find(|context| Some(context.app_id()) == self.focused_app_id.as_deref());
        let mode = active
            .and_then(|context| context.keyboard().on_text_field)
            .or_else(|| global.and_then(|context| context.keyboard().on_text_field))
            .unwrap_or(tehuti_protocol::OnTextField::Button);
        let button = active
            .and_then(|context| context.keyboard().button)
            .or_else(|| global.and_then(|context| context.keyboard().button))
            .unwrap_or(tehuti_protocol::ButtonName::North);
        (mode, button)
    }

    /// Whether the focused context marks a terminal, plus its legacy level-opening option.
    #[must_use]
    pub fn terminal_config(&self) -> (bool, bool) {
        let global = self.contexts.iter().find(|context| context.app_id() == "*");
        let active = self
            .contexts
            .iter()
            .find(|context| Some(context.app_id()) == self.focused_app_id.as_deref());
        let marked = active
            .and_then(|context| context.keyboard().terminal)
            .unwrap_or(false);
        let opens = active
            .and_then(|context| context.keyboard().open_on_app_level)
            .or_else(|| global.and_then(|context| context.keyboard().open_on_app_level))
            .unwrap_or(true);
        (marked, opens)
    }

    /// Configures the launcher names recognized in focused window titles.
    #[must_use]
    pub fn with_bar_agents(mut self, agents: Vec<String>) -> Self {
        self.bar.set_agents(agents);
        self
    }

    /// Swaps the contexts and keeps focus, held buttons and pending plugin decisions.
    pub fn replace_contexts(&mut self, contexts: Vec<Context>) {
        self.contexts = contexts;
        self.refresh_ticker();
    }

    /// Names buttons on the ticker as the pad in use prints them.
    pub fn set_pad_family(&mut self, family: ticker::PadFamily) {
        if family != self.pad_family {
            self.pad_family = family;
            self.refresh_ticker();
        }
    }

    /// Hides a shown mapping ticker or shows a hidden one; the choice outlasts focus changes.
    pub const fn toggle_ticker(&mut self) {
        self.bar.toggle_ticker();
    }

    /// Rebuilds the ticker from the focused context, or disables it there.
    fn refresh_ticker(&mut self) {
        let focused = self.focused_app_id.as_deref();
        let entries = ticker::enabled(&self.contexts, focused)
            .then(|| ticker::entries(&self.contexts, focused, self.pad_family, self.level));
        self.bar.set_ticker(entries);
    }

    /// The installed contexts, in priority order.
    #[must_use]
    pub fn contexts(&self) -> &[Context] {
        &self.contexts
    }

    /// Cancels controller holds when the device disconnects or the seat pauses.
    pub fn controller_disconnected(&mut self) {
        self.guide_held = None;
        self.guide_tap = None;
        self.start_held = false;
        self.menu = None;
        self.menu_buttons.clear();
    }

    /// Releases every key a held button keeps down, each key once.
    pub fn release_held_keys(&mut self) -> Vec<Action> {
        let mut released: Vec<KeyName> = Vec::new();
        for (_, key) in self.held_keys.drain(..) {
            if !released.contains(&key) {
                released.push(key);
            }
        }
        released.into_iter().map(|key| Action::KeyUp { key }).collect()
    }

    /// Ends `button`'s hold; its key goes up once no other held button keeps it down.
    pub fn release_held_button(&mut self, button: ButtonName) -> Option<Action> {
        let index = self.held_keys.iter().position(|(held, _)| *held == button)?;
        let (_, key) = self.held_keys.remove(index);
        (!self.held_keys.iter().any(|(_, other)| *other == key)).then_some(Action::KeyUp { key })
    }

    /// The system menu, for drawing it independently of plugins.
    #[must_use]
    pub const fn menu(&self) -> Option<&menu::Menu> {
        self.menu.as_ref()
    }

    #[must_use]
    pub fn push_to_talk(&self, button: ButtonName) -> bool {
        global_context(self).map_or(button == ButtonName::Record, |context| {
            context.push_to_talk(button)
        })
    }

    /// Whether Guide is down, so Start belongs to the escape chord.
    #[must_use]
    pub const fn guide_held(&self) -> bool {
        self.guide_held.is_some()
    }

    /// Shows on the bar what the on-screen keyboard did with speech.
    pub(crate) fn keyboard_feedback(&mut self, text: &str) {
        self.bar.keyboard_feedback(text);
    }

    /// What the shoulder buttons currently drive.
    #[must_use]
    pub const fn level(&self) -> Level {
        self.level
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
    let released = match event {
        Event::Button {
            name,
            state: ButtonState::Released,
        } => state.release_held_button(*name),
        Event::Button { .. } | Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => None,
    };
    let level = state.level;
    let (mut next, mut actions, routed) = route(state, event, plugin_active);
    if matches!(event, Event::Focus { .. }) || next.level != level {
        next.refresh_ticker();
    }
    if let Some(release) = released {
        actions.insert(0, release);
    }
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
    if menu_button_release(&mut state, event) {
        return (state, Vec::new(), false);
    }
    if let Some(actions) = escape_chord(&mut state, event) {
        return (state, actions, false);
    }
    if state.menu.is_some() {
        let _command = menu_input(&mut state, event);
        return (state, Vec::new(), false);
    }
    if let Some(actions) = shoulder_level(&state, event) {
        return (state, actions, false);
    }
    if state.passthrough() && matches!(event, Event::Button { .. }) {
        return (state, Vec::new(), false);
    }
    if let Some(action) = global_layer(event) {
        let actions = if focused_maps(&state, event) {
            hold_key(&mut state, event).unwrap_or_else(|| active_context(&state, event).into_iter().collect())
        } else {
            vec![action]
        };
        return (state, actions, false);
    }
    if !plugin_active && let Some(actions) = hold_key(&mut state, event) {
        return (state, actions, false);
    }
    if matches!(
        event,
        Event::Button { name, .. } if state.push_to_talk(*name)
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

/// Whether the focused context maps the pressed button, which then replaces the global layer.
fn focused_maps(state: &State, event: &Event) -> bool {
    let Event::Button { name, .. } = event else {
        return false;
    };
    focused_context(state).is_some_and(|context| context.action_for(*name).is_some())
}

/// A `hold = true` mapping presses its key until the button's release.
fn hold_key(state: &mut State, event: &Event) -> Option<Vec<Action>> {
    let Event::Button {
        name,
        state: ButtonState::Pressed,
    } = event
    else {
        return None;
    };
    if state.push_to_talk(*name) {
        return None;
    }
    let key = focused_context(state)
        .filter(|context| context.action_for(*name).is_some())
        .or_else(|| global_context(state))
        .and_then(|context| context.held_key_for(*name))?;
    if state.held_keys.iter().any(|(held, _)| held == name) {
        return Some(Vec::new());
    }
    let already_down = state.held_keys.iter().any(|(_, other)| *other == key);
    state.held_keys.push((*name, key));
    Some(if already_down {
        Vec::new()
    } else {
        vec![Action::KeyDown { key }]
    })
}

/// The global default for South; a focused context that maps South replaces it.
const fn global_layer(event: &Event) -> Option<Action> {
    match *event {
        Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        } => Some(Action::Key {
            key: KeyName::Enter,
            mods: Vec::new(),
        }),
        Event::Button { .. } | Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => None,
    }
}

fn active_context(state: &State, event: &Event) -> Option<Action> {
    if let Event::Speech { text } = event {
        if text.as_str().trim().is_empty() {
            return None;
        }
        let phrase = text.as_str();
        if state.level == Level::App {
            let button = match tehuti_protocol::normalize_phrase(phrase).as_str() {
                "tab left" => Some(ButtonName::LeftBumper),
                "tab right" => Some(ButtonName::RightBumper),
                _ => None,
            };
            if let Some(name) = button {
                return (!state.passthrough())
                    .then(|| {
                        active_context(
                            state,
                            &Event::Button {
                                name,
                                state: ButtonState::Pressed,
                            },
                        )
                    })
                    .flatten();
            }
        }
        let focused = focused_context(state);
        let global = global_context(state);
        return focused
            .and_then(|context| context.mapped_speech_action(phrase))
            .or_else(|| global.and_then(|context| context.mapped_speech_action(phrase)))
            .or_else(|| focused.and_then(|context| context.unmatched_speech_action(phrase)))
            .or_else(|| global.and_then(|context| context.unmatched_speech_action(phrase)))
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

/// In the columns level LB and RB move focus between columns and never reach the app.
fn shoulder_level(state: &State, event: &Event) -> Option<Vec<Action>> {
    let Event::Button { name, state: button } = event else {
        return None;
    };
    let action = match (state.level, name) {
        (Level::Columns, ButtonName::LeftBumper) => LayoutAction::FocusColumnLeft,
        (Level::Columns, ButtonName::RightBumper) => LayoutAction::FocusColumnRight,
        _ => return None,
    };
    Some(match button {
        ButtonState::Pressed => vec![Action::Layout { action }],
        ButtonState::Released => Vec::new(),
    })
}

const fn fallback(_event: &Event) -> Option<Action> {
    None
}

fn menu_button_release(state: &mut State, event: &Event) -> bool {
    if state.menu.is_none()
        && let Event::Button { name, state: button } = event
        && state.menu_buttons.contains(name)
    {
        if *button == ButtonState::Released {
            state.menu_buttons.retain(|held| held != name);
        }
        return true;
    }
    false
}

fn menu_input(state: &mut State, event: &Event) -> Option<menu::Command> {
    if let Event::Button { name, state: button } = event
        && *name != ButtonName::Start
    {
        match button {
            ButtonState::Pressed if !state.menu_buttons.contains(name) => {
                state.menu_buttons.push(*name);
            }
            ButtonState::Released => state.menu_buttons.retain(|held| held != name),
            ButtonState::Pressed => {}
        }
    }
    menu::handle(&mut state.menu, event)
}

fn guide_button(state: &mut State, button_state: ButtonState) {
    match button_state {
        ButtonState::Pressed => {
            if state.menu.take().is_some() {
                state.guide_tap = None;
            } else if state.guide_held.is_none() {
                state.guide_tap = (!state.start_held).then_some(core::time::Duration::ZERO);
            }
            let _held = state.guide_held.get_or_insert(core::time::Duration::ZERO);
        }
        ButtonState::Released => {
            state.guide_held = None;
            if state
                .guide_tap
                .take()
                .is_some_and(|elapsed| elapsed < GUIDE_TAP_WINDOW)
            {
                state.level = state.level.toggled();
                state.bar.set_level(state.level);
            }
        }
    }
}

fn escape_chord(state: &mut State, event: &Event) -> Option<Vec<Action>> {
    match event {
        Event::Button {
            name: ButtonName::Guide,
            state: button_state,
        } => {
            guide_button(state, *button_state);
            Some(Vec::new())
        }
        Event::Button {
            name: ButtonName::Start,
            state: button_state,
        } => {
            state.start_held = *button_state == ButtonState::Pressed;
            if state.guide_held.is_some() {
                state.guide_tap = None;
            }
            state.guide_held.as_mut().map(|held| {
                *held = core::time::Duration::ZERO;
                Vec::new()
            })
        }
        Event::Tick { elapsed } => {
            if let Some(tap) = &mut state.guide_tap {
                *tap = tap.saturating_add(*elapsed);
            }
            if !state.start_held && state.guide_tap.is_some_and(|held| held >= GUIDE_TAP_WINDOW) {
                state.guide_tap = None;
                state.menu = Some(menu::Menu::default());
            }
            if state.start_held
                && let Some(held) = &mut state.guide_held
            {
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
pub mod shadowing;
pub mod speech;

pub mod plugin;

pub mod dispatch;

pub mod controller;

pub mod windows;

pub mod layout;

pub mod bar;
pub mod ticker;

pub mod keyboard;
pub mod menu;
