//! Event routing order: one input in, the next effects out.
//!
//! The compositor answers each query effect with its reply input and sends
//! [`Input::Resume`] after other effects until [`State::busy`] drops back to
//! the depth it started at. Effects may re-enter with a new top-level input;
//! that input runs in its own frame above the waiting one.

use core::time::Duration;
use std::collections::VecDeque;

use tehuti_protocol::{Action, ButtonName, ButtonState, Event, Permission};

mod mouse;
pub use mouse::{PointerButton, Sticks};

use crate::plugin::{Rejection, RequestKey};
use crate::speech::{SpeechEffect, SpeechFailure, SpeechInput, SpeechState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
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
    Speech(SpeechInput),
    PluginReply {
        key: RequestKey,
        result: Result<Vec<Action>, String>,
    },
    PluginExited {
        plugin: usize,
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
    /// Continues the waiting frame after effects without a reply.
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
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
    frames: Vec<Frame>,
}

impl State {
    #[must_use]
    pub fn new(core: crate::State) -> Self {
        Self {
            core,
            speech: SpeechState::default(),
            mouse: mouse::State::default(),
            frames: Vec::new(),
        }
    }

    #[must_use]
    pub const fn core(&self) -> &crate::State {
        &self.core
    }

    pub const fn core_mut(&mut self) -> &mut crate::State {
        &mut self.core
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
    let mut effects = Vec::new();
    let tasks: Vec<Task> = match input {
        Input::Tick { elapsed } => tick_tasks(elapsed).into(),
        Input::Event { event, elapsed } => event_tasks(event, elapsed),
        Input::Speech(input) => vec![Task::Speech(input)],
        Input::PluginExited { plugin } => vec![Task::PluginExited(plugin)],
        Input::PluginReply { key, result } => {
            let (next, actions, rejection) =
                crate::plugin::step(state.core, crate::plugin::Input::Reply { key, result });
            state.core = next;
            effects.extend(actions.into_iter().map(Effect::PluginAction));
            effects.extend(rejection.map(Effect::PluginRejected));
            return (state, effects);
        }
        Input::Sticks(sticks) => {
            state.mouse.sticks = sticks;
            return (state, effects);
        }
        Input::ControllerDisconnected => {
            state.mouse.disconnect(&mut effects);
            state.core.controller_disconnected();
            return (state, effects);
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
        ) => finish_route(&mut state.core, event, actions, route, request, effects),
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

/// Completes routing once the plugin layer has answered.
fn finish_route(
    core_state: &mut crate::State,
    event: Event,
    mut actions: Vec<Action>,
    route: bool,
    request: Option<(RequestKey, Vec<Permission>)>,
    effects: &mut Vec<Effect>,
) {
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
        Task::Route(event) => {
            if let Event::Tick { elapsed } = event
                && state.mouse.tick(elapsed, effects)
            {
                for button in [ButtonState::Released, ButtonState::Pressed] {
                    frame.tasks.push_front(Task::Route(Event::Button {
                        name: ButtonName::Select,
                        state: button,
                    }));
                }
            }
            effects.push(Effect::QueryPlugin(event.clone()));
            frame.awaiting = Some(Await::PluginActive(event));
        }
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

fn controller(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
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
        Event::Button {
            name: ButtonName::Record,
            state: button,
        } => frame.tasks.push_front(Task::Speech(match button {
            ButtonState::Pressed => SpeechInput::Press,
            ButtonState::Released => SpeechInput::Release,
        })),
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
