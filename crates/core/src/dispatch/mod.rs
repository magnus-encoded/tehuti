//! Event routing order: one input in, the next effects out.
//!
//! The compositor answers each query effect with its reply input and sends
//! [`Input::Resume`] after other effects until [`State::busy`] drops back to
//! the depth it started at. Effects may re-enter with a new top-level input;
//! that input runs in its own frame above the waiting one.

use core::time::Duration;
use std::collections::VecDeque;

use tehuti_protocol::{Action, ButtonName, ButtonState, Event, Permission};

mod settings;
pub use settings::Settings;

mod mouse;
pub use mouse::{PointerButton, Sticks};

use crate::plugin::{Rejection, RequestKey};
use crate::speech::{SpeechEffect, SpeechFailure, SpeechInput, SpeechState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    DisplayModes {
        modes: Vec<String>,
        current: usize,
    },
    ModeApplied(Option<usize>),
    DisplayScale(core::num::NonZeroU32),
    /// Elapsed wall time since the previous tick.
    Tick {
        elapsed: Duration,
    },
    /// An event with elapsed time for its leading tick; Focus queries time after routing.
    Event {
        event: Event,
        elapsed: Duration,
    },
    /// Reply to [`Effect::ReadClock`].
    ClockRead(Duration),
    /// Metadata for the same focused surface and application.
    FocusTitle {
        app_id: Option<String>,
        title: String,
    },
    Speech(SpeechInput),
    PluginReply {
        key: RequestKey,
        result: Result<Vec<Action>, String>,
    },
    PluginExited {
        plugin: usize,
    },
    /// A committed text-input enable denotes a new field, even on the same surface.
    TextField {
        active: bool,
        changed: bool,
        kind: crate::keyboard::FieldKind,
    },
    ControllerDisconnected,
    Sticks(Sticks),
    /// Reply to [`Effect::QueryPlugin`].
    PluginActive(bool),
    /// Reply to [`Effect::DispatchPlugin`].
    PluginRequest(Option<(RequestKey, Vec<Permission>)>),
    /// Reply to [`Effect::Supervise`]: the plugins that exited.
    Supervised(Vec<usize>),
    /// Reply to [`Effect::ConfirmSecret`]: whether the confirmation consumed the button.
    SecretConfirmation(bool),
    /// Reply to [`Effect::SpeechCommand`]: whether the sidecar accepted it.
    SpeechSent(bool),
    /// Reply to [`Effect::PollSpeech`].
    SpeechPolled(Vec<SpeechInput>),
    /// Reply to [`Effect::QueryForegroundProgram`].
    ForegroundProgram(bool),
    /// Continues the waiting frame after effects without a reply.
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    PasswordSpeechIgnored,
    RunMenuCommand(crate::menu::Command),
    SettingsMode(usize),
    PersistMode(usize),
    SettingsScale(core::num::NonZeroU32),
    /// Relative motion in millionths of a logical pixel.
    MovePointer {
        x: i32,
        y: i32,
    },
    /// Continuous axis distances in millionths of a logical pixel.
    Scroll {
        x: i32,
        y: i32,
    },
    Click {
        button: PointerButton,
        pressed: bool,
    },
    MouseMode(bool),
    /// Query: sample time after Focus actions. Reply [`Input::ClockRead`].
    ReadClock,
    /// A missing or mismatched query reply aborted the waiting frame.
    InvalidReply,
    /// Query: is a plugin active for this event? Reply [`Input::PluginActive`].
    QueryPlugin(Event),
    /// Query: hand the event to plugins. Reply [`Input::PluginRequest`].
    DispatchPlugin {
        event: Event,
        route: bool,
    },
    /// Query: check plugin health. Reply [`Input::Supervised`].
    Supervise,
    /// Query: offer the South button to a pending secret confirmation. Reply [`Input::SecretConfirmation`].
    ConfirmSecret(ButtonState),
    /// Query: send a start or stop to the speech sidecar. Reply [`Input::SpeechSent`].
    SpeechCommand(SpeechEffect),
    /// Query: read speech sidecar output. Reply [`Input::SpeechPolled`].
    PollSpeech(Duration),
    /// Polls secret lookups, launchers and desktop entries.
    PollSources,
    /// Taps a key with modifiers in the focused client, from the on-screen keyboard.
    Chord(crate::keyboard::Chord),
    /// Query: does the focused terminal's shell have a program in the foreground?
    /// Reply [`Input::ForegroundProgram`].
    QueryForegroundProgram,
    /// Records whether South is held.
    SouthPressed(bool),
    /// Invalidates secret confirmations for the old focus.
    ResetSecretFocus,
    /// Carries out an action from routing.
    Apply(Action),
    /// Carries out a plugin decision or its fallback, unless quitting.
    PluginAction(Action),
    PluginRejected(Rejection),
    /// A transcript became speech; the compositor handles it as a new event.
    Handle(Event),
    SpeechFailed {
        session: u64,
        reason: SpeechFailure,
    },
    TranscriptReceived {
        session: u64,
        text: tehuti_protocol::Text,
        admitted: bool,
    },
    /// Speech routing outcome, for the log.
    SpeechRouted {
        text: tehuti_protocol::Text,
        first: Option<Action>,
        plugin: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Task {
    ReadClock,
    Route(Event),
    Controller(Event),
    Supervise,
    SouthPressed(bool),
    ResetSecretFocus,
    ConfirmSecret(ButtonState, Event),
    Speech(SpeechInput),
    SpeechEffect(SpeechEffect),
    PluginExited(usize),
    PollSpeech(Duration),
    PollSources,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Await {
    ClockRead,
    PluginActive(Event),
    PluginRequest {
        event: Event,
        actions: Vec<Action>,
        route: bool,
    },
    Supervised,
    SecretConfirmation(Event),
    SpeechSent,
    SpeechPolled,
    ForegroundProgram,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Frame {
    tasks: VecDeque<Task>,
    awaiting: Option<Await>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    core: crate::State,
    speech: SpeechState,
    mouse: mouse::State,
    keyboard: crate::keyboard::Keyboard,
    text_field: bool,
    terminal_field: bool,
    opening_button: Option<ButtonName>,
    passthrough_buttons: Vec<ButtonName>,
    passthrough_ptt: bool,
    frames: Vec<Frame>,
    settings: Settings,
}

impl State {
    #[must_use]
    pub fn new(core: crate::State) -> Self {
        Self {
            core,
            speech: SpeechState::default(),
            mouse: mouse::State::default(),
            keyboard: crate::keyboard::Keyboard::default(),
            text_field: false,
            terminal_field: false,
            opening_button: None,
            passthrough_buttons: Vec::new(),
            passthrough_ptt: false,
            frames: Vec::new(),
            settings: Settings::default(),
        }
    }

    #[must_use]
    pub const fn core(&self) -> &crate::State {
        &self.core
    }

    pub const fn core_mut(&mut self) -> &mut crate::State {
        &mut self.core
    }

    /// The on-screen keyboard, for drawing it.
    #[must_use]
    pub const fn keyboard(&self) -> &crate::keyboard::Keyboard {
        &self.keyboard
    }

    #[must_use]
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Frames still waiting for replies or resumption.
    #[must_use]
    pub const fn busy(&self) -> usize {
        self.frames.len()
    }
}

const fn tick_tasks(elapsed: Duration) -> [Task; 4] {
    [
        Task::Route(Event::Tick { elapsed }),
        Task::Supervise,
        Task::PollSpeech(elapsed),
        Task::PollSources,
    ]
}

fn event_tasks(event: Event, elapsed: Duration) -> Vec<Task> {
    if let Event::Tick { elapsed: tick } = event {
        return tick_tasks(tick).into();
    }
    if let Event::Focus { .. } = event {
        let mut tasks = vec![
            Task::Supervise,
            Task::ResetSecretFocus,
            Task::Speech(SpeechInput::Cancel),
            Task::Route(event),
        ];
        tasks.push(Task::ReadClock);
        return tasks;
    }
    let mut tasks: Vec<Task> = tick_tasks(elapsed).into();
    tasks.push(Task::Supervise);
    tasks.push(Task::Controller(event));
    tasks
}

/// Advances routing by one input. Performs no I/O.
#[must_use]
pub fn step(mut state: State, input: Input) -> (State, Vec<Effect>) {
    let menu_open = state.core.menu().is_some();
    let level = state.core.level();
    if matches!(
        &input,
        Input::Event {
            event: Event::Focus { .. },
            ..
        } | Input::ControllerDisconnected
    ) {
        state.passthrough_buttons.clear();
        state.passthrough_ptt = false;
    }
    let (mut state, mut effects) = step_input(state, input);
    release_passthrough_overlays(&mut state, &mut effects);
    if !menu_open && state.core.menu().is_some() {
        state.mouse.release(&mut effects);
        state.keyboard.release_all();
        if let Some(frame) = state.frames.last_mut() {
            frame.tasks.push_front(Task::Speech(SpeechInput::Cancel));
        }
    }
    if level != crate::Level::App && state.core.level() == crate::Level::App {
        sync_terminal(&mut state);
        if state.keyboard.terminal() && state.core.terminal_config().1 {
            state.keyboard.open();
        }
    }
    run(state, effects)
}

/// Picks the terminal layout when the field or the focused app's context says terminal.
fn sync_terminal(state: &mut State) {
    let terminal = state.terminal_field || state.core.terminal_config().0;
    if terminal != state.keyboard.terminal() {
        state.keyboard.set_terminal(terminal);
    }
}

fn step_input(mut state: State, input: Input) -> (State, Vec<Effect>) {
    let mut effects = Vec::new();
    let tasks: Vec<Task> = match input {
        Input::DisplayModes { modes, current } => return display_modes(state, modes, current),
        Input::DisplayScale(scale) => return display_scale(state, scale),
        Input::ModeApplied(current) => return mode_applied(state, current),
        Input::Tick { elapsed } => tick_tasks(elapsed).into(),
        Input::Event { event, elapsed } => event_tasks(event, elapsed),
        Input::FocusTitle { app_id, title } => vec![
            Task::Supervise,
            Task::Route(Event::Focus { app_id, title }),
            Task::ReadClock,
        ],
        Input::Speech(input) => vec![Task::Speech(input)],
        Input::PluginExited { plugin } => vec![Task::PluginExited(plugin)],
        Input::PluginReply { key, result } => return plugin_reply(state, key, result),
        Input::Sticks(sticks) => {
            update_sticks(&mut state, sticks);
            return (state, effects);
        }
        Input::TextField {
            active,
            changed,
            kind,
        } => {
            update_text_field(&mut state, active, changed, kind);
            if state.keyboard.password_field_active() {
                vec![Task::Speech(SpeechInput::Cancel)]
            } else {
                return (state, effects);
            }
        }
        Input::ControllerDisconnected => {
            disconnect_controller(&mut state, &mut effects);
            return (state, effects);
        }
        Input::ForegroundProgram(running) => {
            interrupt_or_close(&mut state, running, &mut effects);
            return run(state, effects);
        }
        reply @ (Input::ClockRead(_)
        | Input::PluginActive(_)
        | Input::PluginRequest(_)
        | Input::Supervised(_)
        | Input::SecretConfirmation(_)
        | Input::SpeechSent(_)
        | Input::SpeechPolled(_)
        | Input::Resume) => {
            resume(&mut state, reply, &mut effects);
            return run(state, effects);
        }
    };
    state.frames.push(Frame {
        tasks: tasks.into(),
        awaiting: None,
    });
    run(state, effects)
}

const fn display_scale(mut state: State, scale: core::num::NonZeroU32) -> (State, Vec<Effect>) {
    state.settings.scale = scale;
    (state, Vec::new())
}

fn mode_applied(mut state: State, current: Option<usize>) -> (State, Vec<Effect>) {
    let mut effects = Vec::new();
    state.settings.applied(current, &mut effects);
    (state, effects)
}

fn plugin_reply(
    mut state: State,
    key: RequestKey,
    result: Result<Vec<Action>, String>,
) -> (State, Vec<Effect>) {
    let (next, actions, rejection) =
        crate::plugin::step(state.core, crate::plugin::Input::Reply { key, result });
    state.core = next;
    let mut effects: Vec<_> = actions.into_iter().map(Effect::PluginAction).collect();
    effects.extend(rejection.map(Effect::PluginRejected));
    (state, effects)
}

fn display_modes(mut state: State, modes: Vec<String>, current: usize) -> (State, Vec<Effect>) {
    state.settings.configure(modes, current);
    (state, Vec::new())
}

fn disconnect_controller(state: &mut State, effects: &mut Vec<Effect>) {
    state.mouse.disconnect(effects);
    state.keyboard.release_all();
    state.opening_button = None;
    state.core.controller_disconnected();
}

fn update_sticks(state: &mut State, sticks: Sticks) {
    if !state.core.passthrough() && state.core.menu().is_none() {
        state.keyboard.left_stick(sticks.left.0);
        state.mouse.sticks = sticks;
    }
}

fn update_text_field(state: &mut State, active: bool, changed: bool, kind: crate::keyboard::FieldKind) {
    state.keyboard.set_field_kind(
        if active {
            kind
        } else {
            crate::keyboard::FieldKind::Normal
        },
        changed,
    );
    state.text_field = active;
    state.terminal_field = active && kind == crate::keyboard::FieldKind::Terminal;
    sync_terminal(state);
    if active
        && changed
        && !state.keyboard.terminal()
        && state.core.keyboard_config().0 == tehuti_protocol::OnTextField::Open
    {
        state.keyboard.open();
    }
}

fn release_passthrough_overlays(state: &mut State, effects: &mut Vec<Effect>) {
    if state.core.passthrough() {
        state.mouse.disconnect(effects);
        state.keyboard.release_all();
        state.keyboard.close();
        state.core.menu = None;
        state.core.guide_tap = None;
        state.settings.close(effects);
    }
}

fn resume(state: &mut State, reply: Input, effects: &mut Vec<Effect>) {
    let Some(frame) = state.frames.last_mut() else {
        return;
    };
    let Some(awaiting) = frame.awaiting.take() else {
        return;
    };
    match (awaiting, reply) {
        (Await::ClockRead, Input::ClockRead(elapsed)) => {
            for task in tick_tasks(elapsed).into_iter().rev() {
                frame.tasks.push_front(task);
            }
        }
        (Await::PluginActive(event), Input::PluginActive(active)) => {
            let (next, actions, route) =
                crate::step_with_plugin(core::mem::take(&mut state.core), &event, active);
            state.core = next;
            effects.push(Effect::DispatchPlugin {
                event: event.clone(),
                route,
            });
            frame.awaiting = Some(Await::PluginRequest {
                event,
                actions,
                route,
            });
        }
        (
            Await::PluginRequest {
                event,
                actions,
                route,
            },
            Input::PluginRequest(request),
        ) => finish_route(state, event, actions, route, request, effects),
        (Await::Supervised, Input::Supervised(exited)) => {
            for plugin in exited.into_iter().rev() {
                frame.tasks.push_front(Task::PluginExited(plugin));
            }
        }
        (Await::SecretConfirmation(event), Input::SecretConfirmation(consumed)) => {
            if !consumed {
                frame.tasks.push_front(Task::Route(event));
            }
        }
        (Await::SpeechSent, Input::SpeechSent(sent)) => {
            if !sent {
                frame.tasks.push_front(Task::Speech(SpeechInput::Unavailable));
            }
        }
        (Await::SpeechPolled, Input::SpeechPolled(inputs)) => {
            for input in inputs.into_iter().rev() {
                frame.tasks.push_front(Task::Speech(input));
            }
        }
        (_, _) => {
            let _aborted = state.frames.pop();
            effects.push(Effect::InvalidReply);
        }
    }
}

/// Reply to B in a terminal: Ctrl-C for a running foreground program, otherwise close.
fn interrupt_or_close(state: &mut State, running: bool, effects: &mut Vec<Effect>) {
    let awaited = state.frames.last_mut().and_then(|frame| {
        frame
            .awaiting
            .take_if(|awaiting| *awaiting == Await::ForegroundProgram)
    });
    if awaited.is_none() {
        resume(state, Input::ForegroundProgram(running), effects);
    } else if running {
        effects.push(Effect::Chord(crate::keyboard::Chord::INTERRUPT));
    } else {
        state.keyboard.close();
    }
}

/// Strips dictation's sentence punctuation from typed speech, for shells.
fn drop_sentence_punctuation(actions: &mut [Action]) {
    for action in actions {
        if let Action::Text { text } = action
            && let Ok(shell) =
                tehuti_protocol::Text::new(crate::keyboard::without_sentence_punctuation(text.as_str()))
        {
            *text = shell;
        }
    }
}

/// Completes routing once the plugin layer has answered.
fn finish_route(
    state: &mut State,
    event: Event,
    mut actions: Vec<Action>,
    route: bool,
    request: Option<(RequestKey, Vec<Permission>)>,
    effects: &mut Vec<Effect>,
) {
    let core_state = &mut state.core;
    if route {
        let (next, mut routed) = match request {
            Some((key, permissions)) => {
                let (next, routed, _) = crate::plugin::step(
                    core::mem::take(core_state),
                    crate::plugin::Input::Request {
                        key,
                        event: event.clone(),
                        permissions,
                    },
                );
                (next, routed)
            }
            None => crate::step(core::mem::take(core_state), &event),
        };
        *core_state = next;
        actions.append(&mut routed);
    }
    if let Event::Speech { text } = event {
        if state.keyboard.terminal() {
            drop_sentence_punctuation(&mut actions);
        }
        effects.push(Effect::SpeechRouted {
            text,
            first: actions.first().cloned(),
            plugin: route,
        });
    }
    effects.extend(actions.into_iter().map(Effect::Apply));
}

/// Runs the top frame until it emits effects, waits for a reply or empties.
fn run(mut state: State, mut effects: Vec<Effect>) -> (State, Vec<Effect>) {
    loop {
        let Some(frame) = state.frames.last_mut() else {
            return (state, effects);
        };
        if frame.awaiting.is_some() {
            return (state, effects);
        }
        if !effects.is_empty() {
            if frame.tasks.is_empty() {
                let _done = state.frames.pop();
            }
            return (state, effects);
        }
        let Some(task) = frame.tasks.pop_front() else {
            let _done = state.frames.pop();
            return (state, effects);
        };
        perform(&mut state, task, &mut effects);
    }
}

/// Advances keyboard repeat and mouse motion; true when a lone View tap is due.
fn tick_pad(
    keyboard: &mut crate::keyboard::Keyboard,
    mouse: &mut mouse::State,
    elapsed: Duration,
    effects: &mut Vec<Effect>,
) -> bool {
    keyboard.tick(elapsed);
    mouse.tick(elapsed, !keyboard.is_open(), effects)
}

fn perform(state: &mut State, task: Task, effects: &mut Vec<Effect>) {
    let Some(frame) = state.frames.last_mut() else {
        return;
    };
    match task {
        Task::ReadClock => {
            effects.push(Effect::ReadClock);
            frame.awaiting = Some(Await::ClockRead);
        }
        Task::Controller(event) => controller(state, event, effects),
        Task::Route(event) => route_event(state, event, effects),
        Task::Supervise => {
            effects.push(Effect::Supervise);
            frame.awaiting = Some(Await::Supervised);
        }
        Task::SouthPressed(pressed) => effects.push(Effect::SouthPressed(pressed)),
        Task::ResetSecretFocus => effects.push(Effect::ResetSecretFocus),
        Task::ConfirmSecret(button, event) => {
            effects.push(Effect::ConfirmSecret(button));
            frame.awaiting = Some(Await::SecretConfirmation(event));
        }
        Task::PollSpeech(elapsed) => {
            effects.push(Effect::PollSpeech(elapsed));
            frame.awaiting = Some(Await::SpeechPolled);
        }
        Task::PollSources => effects.push(Effect::PollSources),
        Task::PluginExited(plugin) => {
            let (next, actions, _) = crate::plugin::step(
                core::mem::take(&mut state.core),
                crate::plugin::Input::Exited { plugin },
            );
            state.core = next;
            effects.extend(actions.into_iter().map(Effect::PluginAction));
        }
        Task::Speech(input) => speech(state, input, effects),
        Task::SpeechEffect(effect) => match effect {
            SpeechEffect::Interim { .. } => {}
            SpeechEffect::Speech { text, .. } => effects.push(Effect::Handle(Event::Speech { text })),
            SpeechEffect::Failed { session, reason } => {
                effects.push(Effect::SpeechFailed { session, reason });
            }
            SpeechEffect::Start { .. } | SpeechEffect::Stop { .. } => {
                effects.push(Effect::SpeechCommand(effect));
                frame.awaiting = Some(Await::SpeechSent);
            }
        },
    }
}

fn route_event(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
    if state.core.passthrough() && matches!(event, Event::Button { .. } | Event::Tick { .. }) {
        let (next, actions) = crate::step(core::mem::take(&mut state.core), &event);
        state.core = next;
        effects.extend(actions.into_iter().map(Effect::Apply));
        return;
    }
    route_tick(state, &event, effects);
    let Some(route_frame) = state.frames.last_mut() else {
        return;
    };
    if let Event::Tick { elapsed } = event
        && state.core.menu().is_none()
        && !state.settings.open
        && tick_pad(&mut state.keyboard, &mut state.mouse, elapsed, effects)
    {
        for button in [ButtonState::Released, ButtonState::Pressed] {
            route_frame.tasks.push_front(Task::Route(Event::Button {
                name: ButtonName::Select,
                state: button,
            }));
        }
    }
    effects.push(Effect::QueryPlugin(event.clone()));
    route_frame.awaiting = Some(Await::PluginActive(event));
}

fn settings_event(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    let was_open = state.settings.open;
    let consumed = state.settings.event(event, effects);
    if !was_open && state.settings.open {
        state.core.menu = None;
        state.mouse.release(effects);
        state.keyboard.release_all();
        state.keyboard.close();
        effects.push(Effect::ResetSecretFocus);
    }
    consumed
}

fn route_tick(state: &mut State, event: &Event, effects: &mut Vec<Effect>) {
    if let Event::Tick { elapsed } = event {
        state.settings.tick(*elapsed, effects);
    }
}

fn system_menu(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if matches!(
        event,
        Event::Button {
            name: ButtonName::Guide,
            ..
        }
    ) {
        if matches!(
            event,
            Event::Button {
                state: ButtonState::Pressed,
                ..
            }
        ) {
            state.settings.close(effects);
        }
        let _response = state.keyboard.button(ButtonName::Guide, start_state(event), true);
        if let Some(actions) = crate::escape_chord(&mut state.core, event) {
            effects.extend(actions.into_iter().map(Effect::Apply));
        }
        return true;
    }
    if state.core.menu().is_some() {
        if let Some(actions) = crate::escape_chord(&mut state.core, event) {
            effects.extend(actions.into_iter().map(Effect::Apply));
        }
        if let Some(command) = crate::menu_input(&mut state.core, event) {
            if command == crate::menu::Command::Settings {
                state.settings.open = true;
                state.settings.display = false;
                effects.push(Effect::ResetSecretFocus);
            } else {
                effects.push(Effect::RunMenuCommand(command));
            }
        }
        return true;
    }
    if state.core.guide_held()
        && matches!(
            event,
            Event::Button {
                name: ButtonName::Start,
                ..
            }
        )
    {
        let _response = state.keyboard.button(ButtonName::Start, start_state(event), true);
        state.keyboard.cancel_send();
        if let Some(actions) = crate::escape_chord(&mut state.core, event) {
            effects.extend(actions.into_iter().map(Effect::Apply));
        }
        return true;
    }
    false
}

fn reserved_input(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if matches!(event, Event::Speech { .. }) && settings_event(state, event, effects) {
        return true;
    }
    sync_terminal(state);
    if crate::menu_button_release(&mut state.core, event) {
        return true;
    }
    if system_menu(state, event, effects) {
        return true;
    }
    if state.keyboard.password_field_active()
        && (matches!(event, Event::Button { name, .. } if state.core.push_to_talk(*name))
            || matches!(event, Event::Speech { .. }))
    {
        effects.push(Effect::PasswordSpeechIgnored);
        return true;
    }

    if settings_event(state, event, effects) {
        return true;
    }
    if keyboard(state, event, effects) {
        return true;
    }
    false
}

fn passthrough_controller(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
    if state.keyboard.password_field_active() && matches!(event, Event::Speech { .. }) {
        effects.push(Effect::PasswordSpeechIgnored);
        return;
    }
    if let Event::Button { name, state: button } = &event {
        state.passthrough_buttons.retain(|held| held != name);
        if *button == ButtonState::Pressed {
            state.passthrough_buttons.push(*name);
        }
        let held = state
            .core
            .passthrough_push_to_talk()
            .iter()
            .all(|member| state.passthrough_buttons.contains(member));
        if held != state.passthrough_ptt {
            state.passthrough_ptt = held;
            if state.keyboard.password_field_active() {
                effects.push(Effect::PasswordSpeechIgnored);
                return;
            }
            speech(
                state,
                if held {
                    SpeechInput::Press
                } else {
                    SpeechInput::Release
                },
                effects,
            );
        }
    }
    route_event(state, event, effects);
}

fn controller(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
    if state.core.passthrough() {
        passthrough_controller(state, event, effects);
        return;
    }
    if reserved_input(state, &event, effects) {
        return;
    }
    let Some(frame) = state.frames.last_mut() else {
        return;
    };
    if state.mouse.button(&event, effects) {
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::MouseMode(_)))
        {
            frame.tasks.push_front(Task::Speech(SpeechInput::Cancel));
        }
        if let Some(actions) = crate::escape_chord(&mut state.core, &event) {
            effects.extend(actions.into_iter().map(Effect::Apply));
        }
        return;
    }
    match event {
        Event::Button { name, state: button } if state.core.push_to_talk(name) => {
            frame.tasks.push_front(Task::Speech(match button {
                ButtonState::Pressed => SpeechInput::Press,
                ButtonState::Released => SpeechInput::Release,
            }));
        }
        Event::Button {
            name: ButtonName::South,
            state: button,
        } => {
            frame.tasks.push_front(Task::ConfirmSecret(button, event));
            frame
                .tasks
                .push_front(Task::SouthPressed(button == ButtonState::Pressed));
        }
        Event::Button { .. } | Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => {
            frame.tasks.push_front(Task::Route(event));
        }
    }
}

/// The open keyboard takes speech as dictation and most buttons; "keyboard" opens it.
fn keyboard(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if let Event::Button { name, state: button } = event {
        if state.opening_button == Some(*name) {
            if *button == ButtonState::Released {
                state.opening_button = None;
            }
            return true;
        }
        let (mode, opening_button) = state.core.keyboard_config();
        if !state.keyboard.is_open()
            && state.text_field
            && !state.keyboard.terminal()
            && mode != tehuti_protocol::OnTextField::Off
            && *name == opening_button
            && *button == ButtonState::Pressed
        {
            state.opening_button = Some(*name);
            state.keyboard.open();
            return true;
        }
    }
    match event {
        Event::Speech { text } if state.keyboard.is_open() => {
            state.keyboard.dictate(text.as_str());
            state.core.keyboard_feedback("Dictated");
            true
        }
        Event::Speech { text } if crate::keyboard::is_open_phrase(text.as_str()) => {
            state.keyboard.open();
            state.core.keyboard_feedback("Keyboard");
            true
        }
        Event::Button {
            name: ButtonName::Start,
            ..
        } if state.keyboard.is_open() || state.keyboard.holds(ButtonName::Start) => {
            if let Some(actions) = crate::escape_chord(&mut state.core, event) {
                effects.extend(actions.into_iter().map(Effect::Apply));
            }
            let guide_held = state.core.guide_held();
            let response = state.keyboard.button(ButtonName::Start, start_state(event), true);
            if guide_held {
                state.keyboard.cancel_send();
            }
            send_response(state, response, effects)
        }
        Event::Button { name, state: button } => {
            let shoulders_move_caret = state.core.level() == crate::Level::App;
            let response = state.keyboard.button(*name, *button, shoulders_move_caret);
            send_response(state, response, effects)
        }
        Event::Speech { .. } | Event::Focus { .. } | Event::Tick { .. } => false,
    }
}

const fn start_state(event: &Event) -> ButtonState {
    match event {
        Event::Button { state, .. } => *state,
        Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => ButtonState::Released,
    }
}

/// True when the keyboard consumed the button; a send becomes a text action.
fn send_response(state: &mut State, response: crate::keyboard::Response, effects: &mut Vec<Effect>) -> bool {
    match response {
        crate::keyboard::Response::Pass => false,
        crate::keyboard::Response::Consumed => true,
        crate::keyboard::Response::Send(draft) => {
            if let Ok(text) = tehuti_protocol::Text::new(draft) {
                effects.push(Effect::Apply(Action::Text { text }));
            }
            true
        }
        crate::keyboard::Response::Chord(draft, chord) => {
            if !draft.is_empty()
                && let Ok(text) = tehuti_protocol::Text::new(draft)
            {
                effects.push(Effect::Apply(Action::Text { text }));
            }
            effects.push(Effect::Chord(chord));
            true
        }
        crate::keyboard::Response::InterruptOrClose => {
            if let Some(frame) = state.frames.last_mut() {
                effects.push(Effect::QueryForegroundProgram);
                frame.awaiting = Some(Await::ForegroundProgram);
            }
            true
        }
    }
}

fn speech(state: &mut State, input: SpeechInput, effects: &mut Vec<Effect>) {
    let transcript = if let SpeechInput::Transcript { session, text } = &input {
        Some((*session, text.clone()))
    } else {
        None
    };
    let (next, speech) = state.core.capture(core::mem::take(&mut state.speech), input);
    state.speech = next;
    if let Some((session, text)) = transcript {
        effects.push(Effect::TranscriptReceived {
            session,
            text,
            admitted: !speech.is_empty(),
        });
    }
    for effect in speech.into_iter().rev() {
        if let Some(frame) = state.frames.last_mut() {
            frame.tasks.push_front(Task::SpeechEffect(effect));
        }
    }
}

#[cfg(test)]
mod tests;
