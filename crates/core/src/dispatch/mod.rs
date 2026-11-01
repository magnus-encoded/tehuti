//! Event routing order: one input in, the next effects out.
//!
//! The compositor answers each query effect with its reply input and sends
//! [`Input::Resume`] after other effects until [`State::busy`] drops back to
//! the depth it started at. Effects may re-enter with a new top-level input;
//! that input runs in its own frame above the waiting one.

use crate::overlay::{Overlay, STACK};
use core::time::Duration;
use std::collections::VecDeque;

use tehuti_protocol::{Action, ButtonName, ButtonState, Event, Permission};

mod input_settings;
pub use input_settings::{
    ControllerChoice, ControllerInfo, InputRow, InputSettings, Microphone, MicrophoneChoice,
};

mod settings;
pub use settings::{Category, Page, Settings};

mod mouse;
pub use mouse::{PointerButton, Sticks};

use crate::plugin::{Rejection, RequestKey};
use crate::speech::{SpeechEffect, SpeechFailure, SpeechInput, SpeechState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Menu(crate::menu::Command),
    Launcher(crate::launcher::Launcher),
    /// Closes the session-start invitation once a window exists.
    CloseInvitation,
    ChoiceList(crate::choice_list::Slots),
    DisplayModes {
        modes: Vec<String>,
        current: usize,
    },
    ModeApplied(Option<usize>),
    WallpaperSaved(usize),
    DisplayScale(core::num::NonZeroU32),
    BarFont(crate::bar::Font),
    Wallpapers {
        names: Vec<String>,
        locked: Vec<bool>,
        current: usize,
    },
    /// The connected pads and their labels, in device path order.
    Controllers(Vec<ControllerInfo>),
    /// Reply to [`Effect::ListMicrophones`]: the capture sources found.
    Microphones(Vec<Microphone>),
    /// The saved input choices, read at startup.
    InputChoices {
        controller: ControllerChoice,
        microphone: MicrophoneChoice,
    },
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
    /// Whether the compositor currently presents a pending secret confirmation.
    SecretPrompt(bool),
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
    ChooseLauncher(usize),
    OpenLauncher(crate::unit_verbs::LauncherRequest),
    PasswordSpeechIgnored,
    RunMenuCommand(crate::menu::Command),
    SettingsMode(usize),
    PersistMode(usize),
    SettingsScale(core::num::NonZeroU32),
    SettingsBarFont(crate::bar::Font),
    WallpaperPreview(usize),
    WallpaperSave(usize),
    UnlockWallpapers,
    /// Routes pad input from the chosen controller only, until confirmed or reverted.
    SettingsController(ControllerChoice),
    PersistController(ControllerChoice),
    /// Saves the capture sources the speech sidecar records from.
    SettingsMicrophone(MicrophoneChoice),
    /// Lists capture sources; the compositor answers later with [`Input::Microphones`].
    ListMicrophones,
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
    /// Query: offer the confirm button to a pending secret confirmation. Reply [`Input::SecretConfirmation`].
    ConfirmSecret(ButtonState),
    /// Query: send a start or stop to the speech sidecar. Reply [`Input::SpeechSent`].
    SpeechCommand(SpeechEffect),
    /// Query: read speech sidecar output. Reply [`Input::SpeechPolled`].
    PollSpeech(Duration),
    /// Polls secret lookups, launchers and desktop entries.
    PollSources,
    /// Taps a key with modifiers in the focused client, from the on-screen keyboard.
    Chord(crate::keyboard::Chord),
    /// Records whether confirm is held.
    ConfirmPressed(bool),
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
        /// The phrase the transcript matched only fuzzily, when that match routed it.
        fuzzy: Option<tehuti_protocol::FuzzyMatch>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Task {
    ReadClock,
    Route(Event),
    Controller(Event),
    Supervise,
    ConfirmPressed(bool),
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
enum Takeback {
    #[default]
    Idle,
    Holding(Duration),
    Fired,
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
    takeback: Takeback,
    frames: Vec<Frame>,
    settings: Settings,
    launcher: Option<crate::launcher::Launcher>,
    choice_list: Option<crate::choice_list::List>,
    choice_buttons: Vec<ButtonName>,
    desktop_buttons: Vec<ButtonName>,
    desktop_selection: usize,
    overlay_stack: Vec<Overlay>,
    secret_prompt: SecretPrompt,
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
            takeback: Takeback::Idle,
            frames: Vec::new(),
            settings: Settings::default(),
            launcher: None,
            choice_list: None,
            choice_buttons: Vec::new(),
            desktop_buttons: Vec::new(),
            desktop_selection: 0,
            overlay_stack: Vec::new(),
            secret_prompt: SecretPrompt::Hidden,
        }
    }

    #[must_use]
    pub const fn core(&self) -> &crate::State {
        &self.core
    }

    pub const fn core_mut(&mut self) -> &mut crate::State {
        &mut self.core
    }

    /// The selected wallpaper launch hint while the desktop owns input.
    #[must_use]
    pub fn desktop_focus(&self) -> Option<usize> {
        let entries = desktop_favourites(self);
        (self.core.focused_app_id.is_none() && open_overlays(self).is_empty() && !entries.is_empty())
            .then(|| self.desktop_selection.min(entries.len().saturating_sub(1)))
    }

    /// The on-screen keyboard, for drawing it.
    #[must_use]
    pub const fn keyboard(&self) -> &crate::keyboard::Keyboard {
        &self.keyboard
    }

    /// Drains the keyboard's explicit live replacements.
    pub fn take_keyboard_edits(&mut self) -> Vec<crate::keyboard::Edit> {
        self.keyboard.take_edits()
    }

    /// Records delivery of the keyboard's live edit.
    pub const fn keyboard_edit_applied(&mut self) {
        self.keyboard.edit_applied();
    }

    /// Applies the application's committed surrounding text to the keyboard.
    pub fn set_keyboard_surrounding(&mut self, surrounding: Option<(String, usize, usize)>) {
        self.keyboard.set_surrounding(surrounding);
    }

    #[must_use]
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }

    #[must_use]
    pub const fn secret_prompt_visible(&self) -> bool {
        matches!(self.secret_prompt, SecretPrompt::Visible)
    }

    /// The open launcher, if the open list is one.
    #[must_use]
    pub fn launcher(&self) -> Option<&crate::launcher::Launcher> {
        self.launcher
            .as_ref()
            .filter(|launcher| launcher.list().is_open())
    }

    #[must_use]
    pub fn choice_list(&self) -> Option<&crate::choice_list::List> {
        self.launcher
            .as_ref()
            .map(crate::launcher::Launcher::list)
            .filter(|list| list.is_open())
            .or(self.choice_list.as_ref())
    }

    fn choice_list_open(&self) -> bool {
        self.choice_list().is_some_and(crate::choice_list::List::is_open)
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum SecretPrompt {
    #[default]
    Hidden,
    Visible,
}

/// Advances routing by one input. Performs no I/O.
#[must_use]
pub fn step(mut state: State, input: Input) -> (State, Vec<Effect>) {
    let mut released = Vec::new();
    if matches!(
        &input,
        Input::Event {
            event: Event::Focus { .. },
            ..
        } | Input::ControllerDisconnected
    ) {
        state.core.pad_taken_back = false;
        state.takeback = Takeback::Idle;
        state.passthrough_buttons.clear();
        state.passthrough_ptt = false;
        release_held_keys(&mut state, &mut released);
    }
    let (mut state, mut routed) = step_input(state, input);
    let mut effects = released;
    effects.append(&mut routed);
    release_on_open(&mut state, &mut effects);
    release_passthrough_overlays(&mut state, &mut effects);
    state.overlay_stack = open_overlays(&state);
    run(state, effects)
}

/// Applies the focused context’s draft retention and terminal layout.
fn sync_keyboard_config(state: &mut State) {
    state.keyboard.set_pad_family(state.core.pad_family());
    state.settings.pad_family = state.core.pad_family();
    state.mouse.pad_family = state.core.pad_family();
    state.keyboard.set_keep_draft(state.core.keep_keyboard_draft());
    let terminal = state.terminal_field || state.core.terminal_config().0;
    if terminal != state.keyboard.terminal() {
        state.keyboard.set_terminal(terminal);
    }
}

fn step_input(mut state: State, input: Input) -> (State, Vec<Effect>) {
    match input {
        Input::Menu(command) => {
            let mut effects = Vec::new();
            release_pad(&mut state, Handoff::Open(Overlay::SystemMenu), &mut effects);
            state.core.menu = Some(crate::menu::Menu::for_command(state.core.pad_family(), command));
            (state, effects)
        }
        Input::Launcher(launcher) => {
            state.choice_list = None;
            state.launcher = Some(launcher);
            let mut effects = Vec::new();
            release_pad(&mut state, Handoff::Open(Overlay::ChoiceList), &mut effects);
            (state, effects)
        }
        Input::CloseInvitation => {
            state.launcher = state.launcher.filter(|launcher| !launcher.is_invitation());
            (state, Vec::new())
        }
        Input::ChoiceList(slots) => {
            state.launcher = None;
            state.choice_list = Some(crate::choice_list::List::new(slots));
            (state, Vec::new())
        }
        input @ (Input::DisplayModes { .. }
        | Input::BarFont(_)
        | Input::DisplayScale(_)
        | Input::Wallpapers { .. }
        | Input::WallpaperSaved(_)
        | Input::ModeApplied(_)
        | Input::Controllers(_)
        | Input::Microphones(_)
        | Input::InputChoices { .. }) => settings_input(state, input),
        input @ (Input::Tick { .. }
        | Input::Event { .. }
        | Input::FocusTitle { .. }
        | Input::Speech(_)
        | Input::PluginExited { .. }) => frame_input(state, input),
        input @ (Input::SecretPrompt(_)
        | Input::Sticks(_)
        | Input::TextField { .. }
        | Input::ControllerDisconnected) => state_input(state, &input),
        Input::PluginReply { key, result } => plugin_reply(state, key, result),
        reply @ (Input::ClockRead(_)
        | Input::PluginActive(_)
        | Input::PluginRequest(_)
        | Input::Supervised(_)
        | Input::SecretConfirmation(_)
        | Input::SpeechSent(_)
        | Input::SpeechPolled(_)
        | Input::Resume) => {
            let mut effects = Vec::new();
            resume(&mut state, reply, &mut effects);
            run(state, effects)
        }
    }
}

fn frame_input(state: State, input: Input) -> (State, Vec<Effect>) {
    let tasks = match input {
        Input::Tick { elapsed } => tick_tasks(elapsed).into(),
        Input::Event { event, elapsed } => event_tasks(event, elapsed),
        Input::FocusTitle { app_id, title } => vec![
            Task::Supervise,
            Task::Route(Event::Focus { app_id, title }),
            Task::ReadClock,
        ],
        Input::Speech(input) => vec![Task::Speech(input)],
        Input::PluginExited { plugin } => vec![Task::PluginExited(plugin)],
        Input::Menu(_)
        | Input::Launcher(_)
        | Input::CloseInvitation
        | Input::ChoiceList(_)
        | Input::DisplayModes { .. }
        | Input::Wallpapers { .. }
        | Input::WallpaperSaved(_)
        | Input::BarFont(_)
        | Input::DisplayScale(_)
        | Input::ModeApplied(_)
        | Input::Controllers(_)
        | Input::Microphones(_)
        | Input::InputChoices { .. }
        | Input::SecretPrompt(_)
        | Input::Sticks(_)
        | Input::TextField { .. }
        | Input::ControllerDisconnected
        | Input::PluginReply { .. }
        | Input::ClockRead(_)
        | Input::PluginActive(_)
        | Input::PluginRequest(_)
        | Input::Supervised(_)
        | Input::SecretConfirmation(_)
        | Input::SpeechSent(_)
        | Input::SpeechPolled(_)
        | Input::Resume => Vec::new(),
    };
    run_tasks(state, tasks, Vec::new())
}

fn state_input(mut state: State, input: &Input) -> (State, Vec<Effect>) {
    let mut effects = Vec::new();
    match input {
        Input::SecretPrompt(visible) => {
            state.secret_prompt = if *visible {
                SecretPrompt::Visible
            } else {
                SecretPrompt::Hidden
            }
        }
        Input::Sticks(sticks) => update_sticks(&mut state, *sticks),
        Input::TextField {
            active,
            changed,
            kind,
        } => {
            update_text_field(&mut state, *active, *changed, *kind);
            if !active {
                state.keyboard.set_surrounding(None);
            }
            if state.keyboard.password_field_active() {
                return run_tasks(state, vec![Task::Speech(SpeechInput::Cancel)], effects);
            }
        }
        Input::ControllerDisconnected => disconnect_controller(&mut state, &mut effects),
        Input::Menu(_)
        | Input::Launcher(_)
        | Input::CloseInvitation
        | Input::ChoiceList(_)
        | Input::DisplayModes { .. }
        | Input::Wallpapers { .. }
        | Input::WallpaperSaved(_)
        | Input::BarFont(_)
        | Input::DisplayScale(_)
        | Input::ModeApplied(_)
        | Input::Controllers(_)
        | Input::Microphones(_)
        | Input::InputChoices { .. }
        | Input::Tick { .. }
        | Input::Event { .. }
        | Input::FocusTitle { .. }
        | Input::Speech(_)
        | Input::PluginExited { .. }
        | Input::PluginReply { .. }
        | Input::ClockRead(_)
        | Input::PluginActive(_)
        | Input::PluginRequest(_)
        | Input::Supervised(_)
        | Input::SecretConfirmation(_)
        | Input::SpeechSent(_)
        | Input::SpeechPolled(_)
        | Input::Resume => {}
    }
    (state, effects)
}

fn run_tasks(mut state: State, tasks: Vec<Task>, effects: Vec<Effect>) -> (State, Vec<Effect>) {
    state.frames.push(Frame {
        tasks: tasks.into(),
        awaiting: None,
    });
    run(state, effects)
}

/// Inputs that only update the settings view.
fn settings_input(mut state: State, input: Input) -> (State, Vec<Effect>) {
    let mut effects = Vec::new();
    match input {
        Input::DisplayModes { modes, current } => state.settings.configure(modes, current),
        Input::BarFont(font) => state.settings.bar_font = font,
        Input::DisplayScale(scale) => state.settings.scale = scale,
        Input::Wallpapers {
            names,
            locked,
            current,
        } => state.settings.configure_wallpapers(names, locked, current),
        Input::WallpaperSaved(index) => state.settings.wallpaper_saved(index),
        Input::ModeApplied(current) => state.settings.applied(current, &mut effects),
        Input::Controllers(names) => state.settings.configure_controllers(names),
        Input::Microphones(microphones) => state.settings.configure_microphones(microphones),
        Input::InputChoices {
            controller,
            microphone,
        } => state.settings.restore_input(controller, microphone),
        Input::Menu(_)
        | Input::Launcher(_)
        | Input::CloseInvitation
        | Input::ChoiceList(_)
        | Input::Tick { .. }
        | Input::Event { .. }
        | Input::ClockRead(_)
        | Input::Speech(_)
        | Input::PluginReply { .. }
        | Input::PluginExited { .. }
        | Input::TextField { .. }
        | Input::ControllerDisconnected
        | Input::SecretPrompt(_)
        | Input::Sticks(_)
        | Input::PluginActive(_)
        | Input::PluginRequest(_)
        | Input::Supervised(_)
        | Input::SecretConfirmation(_)
        | Input::SpeechSent(_)
        | Input::SpeechPolled(_)
        | Input::FocusTitle { .. }
        | Input::Resume => {}
    }
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

fn disconnect_controller(state: &mut State, effects: &mut Vec<Effect>) {
    release_pad(state, Handoff::Disconnect, effects);
    state.opening_button = None;
    state.choice_buttons.clear();
    state.core.controller_disconnected();
}

fn update_sticks(state: &mut State, sticks: Sticks) {
    if !state.choice_list_open() && !state.core.passthrough() && state.core.menu().is_none() {
        state.keyboard.left_stick(sticks.left.0, sticks.left.1);
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
    sync_keyboard_config(state);
}

/// Lifts every key a held pad button keeps down.
fn release_held_keys(state: &mut State, effects: &mut Vec<Effect>) {
    effects.extend(state.core.release_held_keys().into_iter().map(Effect::Apply));
}

#[derive(Clone, Copy)]
enum Handoff {
    Open(Overlay),
    Disconnect,
    Passthrough,
    Takeback,
}

fn release_pad(state: &mut State, handoff: Handoff, effects: &mut Vec<Effect>) {
    let owner = match handoff {
        Handoff::Open(owner) => owner,
        Handoff::Disconnect | Handoff::Passthrough | Handoff::Takeback => Overlay::Settings,
    };
    for layer in owner.below() {
        release_layer(state, layer, handoff, effects);
    }
    finish_handoff(state, handoff, effects);
}

fn release_layer(state: &mut State, layer: Overlay, handoff: Handoff, effects: &mut Vec<Effect>) {
    match layer {
        Overlay::Keyboard => {
            if state.keyboard.talking()
                && let Some(frame) = state.frames.last_mut()
            {
                frame.tasks.push_front(Task::Speech(SpeechInput::Cancel));
            }
            state.keyboard.release_all();
            if matches!(handoff, Handoff::Open(Overlay::Settings) | Handoff::Passthrough) {
                state.keyboard.close();
            }
        }
        Overlay::Mouse => release_pointer(state, handoff, effects),
        Overlay::Application => release_held_keys(state, effects),
        Overlay::Cursor
        | Overlay::Settings
        | Overlay::ChoiceList
        | Overlay::SystemMenu
        | Overlay::Secret
        | Overlay::SecretUnavailable
        | Overlay::Notice
        | Overlay::MouseNotice
        | Overlay::Bar => {}
    }
}

fn release_pointer(state: &mut State, handoff: Handoff, effects: &mut Vec<Effect>) {
    match handoff {
        Handoff::Disconnect | Handoff::Passthrough => state.mouse.disconnect(effects),
        Handoff::Open(_) | Handoff::Takeback => state.mouse.release(effects),
    }
}

fn finish_handoff(state: &mut State, handoff: Handoff, effects: &mut Vec<Effect>) {
    match handoff {
        Handoff::Open(Overlay::Settings) => {
            state.core.menu = None;
            effects.push(Effect::ResetSecretFocus);
        }
        Handoff::Passthrough => {
            state.settings.close(effects);
            if let Some(list) = &mut state.choice_list {
                list.cancel();
            }
        }
        Handoff::Open(Overlay::SystemMenu) => {
            if let Some(frame) = state.frames.last_mut() {
                frame.tasks.push_front(Task::Speech(SpeechInput::Cancel));
            }
        }
        Handoff::Open(_) | Handoff::Disconnect | Handoff::Takeback => {}
    }
}

fn release_passthrough_overlays(state: &mut State, effects: &mut Vec<Effect>) {
    if state.core.passthrough() {
        release_pad(state, Handoff::Passthrough, effects);
    }
}

fn open_overlays(state: &State) -> Vec<Overlay> {
    STACK
        .into_iter()
        .filter(|layer| match layer {
            Overlay::Settings => state.settings.open,
            Overlay::ChoiceList => state.choice_list_open(),
            Overlay::SystemMenu => state.core.menu().is_some(),
            Overlay::Secret => state.secret_prompt == SecretPrompt::Visible,
            Overlay::Keyboard => state.keyboard.is_open(),
            Overlay::Mouse => state.mouse.active(),
            Overlay::Cursor
            | Overlay::SecretUnavailable
            | Overlay::Notice
            | Overlay::MouseNotice
            | Overlay::Bar
            | Overlay::Application => false,
        })
        .collect()
}

fn release_on_open(state: &mut State, effects: &mut Vec<Effect>) {
    for owner in open_overlays(state) {
        if !state.overlay_stack.contains(&owner) {
            release_pad(state, Handoff::Open(owner), effects);
        }
    }
    state.overlay_stack = open_overlays(state);
}

fn resume(state: &mut State, reply: Input, effects: &mut Vec<Effect>) {
    let Some(frame) = state.frames.last_mut() else {
        return;
    };
    let Some(awaiting) = frame.awaiting.take() else {
        return;
    };
    match (awaiting, reply) {
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
        pair @ ((Await::ClockRead, Input::ClockRead(_))
        | (Await::Supervised, Input::Supervised(_))
        | (Await::SpeechPolled, Input::SpeechPolled(_))
        | (Await::SecretConfirmation(_), Input::SecretConfirmation(_))
        | (Await::SpeechSent, Input::SpeechSent(_))) => queue_reply(frame, pair),
        (_, _) => {
            let _aborted = state.frames.pop();
            effects.push(Effect::InvalidReply);
        }
    }
}

fn queue_reply(frame: &mut Frame, pair: (Await, Input)) {
    match pair {
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
        pair => queue_batch(frame, pair),
    }
}

fn queue_batch(frame: &mut Frame, pair: (Await, Input)) {
    match pair {
        (Await::ClockRead, Input::ClockRead(elapsed)) => {
            for task in tick_tasks(elapsed).into_iter().rev() {
                frame.tasks.push_front(task);
            }
        }
        (Await::Supervised, Input::Supervised(exited)) => {
            for plugin in exited.into_iter().rev() {
                frame.tasks.push_front(Task::PluginExited(plugin));
            }
        }
        (Await::SpeechPolled, Input::SpeechPolled(inputs)) => {
            for input in inputs.into_iter().rev() {
                frame.tasks.push_front(Task::Speech(input));
            }
        }
        (_, _) => {}
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
        let first = actions.first().cloned();
        let fuzzy = state
            .core
            .fuzzy_speech_match(text.as_str())
            .filter(|matched| first.as_ref() == Some(&matched.action));
        effects.push(Effect::SpeechRouted {
            text,
            first,
            plugin: route,
            fuzzy,
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
        release_on_open(&mut state, &mut effects);
    }
}

fn perform(state: &mut State, task: Task, effects: &mut Vec<Effect>) {
    match task {
        task @ (Task::ReadClock | Task::Supervise | Task::ConfirmSecret(_, _) | Task::PollSpeech(_)) => {
            query_task(state, task, effects);
        }
        task @ (Task::Speech(_) | Task::SpeechEffect(_)) => speech_task(state, task, effects),
        Task::Controller(event) => controller(state, event, effects),
        Task::Route(event) => route_event(state, event, effects),
        Task::ConfirmPressed(pressed) => effects.push(Effect::ConfirmPressed(pressed)),
        Task::ResetSecretFocus => effects.push(Effect::ResetSecretFocus),
        Task::PollSources => effects.push(Effect::PollSources),
        Task::PluginExited(plugin) => {
            let (next, actions, _) = crate::plugin::step(
                core::mem::take(&mut state.core),
                crate::plugin::Input::Exited { plugin },
            );
            state.core = next;
            effects.extend(actions.into_iter().map(Effect::PluginAction));
        }
    }
}

fn query_task(state: &mut State, task: Task, effects: &mut Vec<Effect>) {
    let Some(frame) = state.frames.last_mut() else {
        return;
    };
    match task {
        Task::ReadClock => {
            effects.push(Effect::ReadClock);
            frame.awaiting = Some(Await::ClockRead);
        }
        Task::Supervise => {
            effects.push(Effect::Supervise);
            frame.awaiting = Some(Await::Supervised);
        }
        Task::ConfirmSecret(button, event) => {
            effects.push(Effect::ConfirmSecret(button));
            frame.awaiting = Some(Await::SecretConfirmation(event));
        }
        Task::PollSpeech(elapsed) => {
            effects.push(Effect::PollSpeech(elapsed));
            frame.awaiting = Some(Await::SpeechPolled);
        }
        Task::Controller(_)
        | Task::Route(_)
        | Task::ConfirmPressed(_)
        | Task::ResetSecretFocus
        | Task::PollSources
        | Task::PluginExited(_)
        | Task::Speech(_)
        | Task::SpeechEffect(_) => {}
    }
}

fn speech_task(state: &mut State, task: Task, effects: &mut Vec<Effect>) {
    match task {
        Task::Speech(input) => speech(state, input, effects),
        Task::SpeechEffect(effect) => speech_effect(state, effect, effects),
        Task::ReadClock
        | Task::Supervise
        | Task::ConfirmSecret(_, _)
        | Task::PollSpeech(_)
        | Task::Controller(_)
        | Task::Route(_)
        | Task::ConfirmPressed(_)
        | Task::ResetSecretFocus
        | Task::PollSources
        | Task::PluginExited(_) => {}
    }
}

fn speech_effect(state: &mut State, effect: SpeechEffect, effects: &mut Vec<Effect>) {
    match effect {
        SpeechEffect::Interim { .. } => {}
        SpeechEffect::Speech { text, .. } => effects.push(Effect::Handle(Event::Speech { text })),
        SpeechEffect::Failed { session, reason } => effects.push(Effect::SpeechFailed { session, reason }),
        SpeechEffect::Start { .. } | SpeechEffect::Stop { .. } => {
            if let Some(launcher) = &mut state.launcher {
                launcher.stop_timer();
            }
            effects.push(Effect::SpeechCommand(effect));
            if let Some(frame) = state.frames.last_mut() {
                frame.awaiting = Some(Await::SpeechSent);
            }
        }
    }
}

fn route_event(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
    if launcher_event(state, &event, effects) {
        return;
    }
    if let Event::Speech { text } = &event
        && let Some(request) = crate::unit_verbs::launcher_request(text.as_str())
    {
        effects.push(Effect::OpenLauncher(request));
        return;
    }
    if let Event::Tick { elapsed } = &event
        && let Takeback::Holding(held) = &mut state.takeback
    {
        *held = held.saturating_add(*elapsed);
        if *held >= Duration::from_millis(500) {
            state.takeback = Takeback::Fired;
            state.core.pad_taken_back = !state.core.pad_taken_back;
            release_pad(state, Handoff::Takeback, effects);
        }
    }
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
        && state.choice_list.as_ref().is_none_or(|list| !list.is_open())
    {
        if state.keyboard.tick(elapsed) {
            route_frame.tasks.push_front(Task::Speech(SpeechInput::Press));
        }
        if state.mouse.tick(elapsed, !state.keyboard.is_open(), effects) {
            for button in [ButtonState::Released, ButtonState::Pressed] {
                route_frame.tasks.push_front(Task::Route(Event::Button {
                    name: ButtonName::Select,
                    state: button,
                }));
            }
        }
    }
    effects.push(Effect::QueryPlugin(event.clone()));
    route_frame.awaiting = Some(Await::PluginActive(event));
}

fn settings_event(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    state.settings.event(event, effects)
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
        guide_input(state, event, effects);
        return true;
    }
    if state.core.menu().is_some() {
        if let Some(actions) = crate::escape_chord(&mut state.core, event) {
            effects.extend(actions.into_iter().map(Effect::Apply));
        }
        if let Some(command) = crate::menu_input(&mut state.core, event) {
            if command == crate::menu::Command::Settings {
                state.settings.open = true;
                state.settings.page = Page::Categories;
            } else {
                effects.push(Effect::RunMenuCommand(command));
            }
        }
        return true;
    }
    if (state.core.guide_held() && matches!(start_state(event), ButtonState::Pressed)
        || state.core.start_in_chord)
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

fn guide_input(state: &mut State, event: &Event, effects: &mut Vec<Effect>) {
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
}

fn reserved_input(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if let Event::Button {
        name,
        state: ButtonState::Released,
    } = event
        && (state.choice_buttons.contains(name) || state.desktop_buttons.contains(name))
    {
        state.choice_buttons.retain(|held| held != name);
        state.desktop_buttons.retain(|held| held != name);
        return true;
    }
    sync_keyboard_config(state);
    let dictation = state.keyboard.is_open() && matches!(event, Event::Speech { .. });
    if settings_phrase(state, event, dictation, effects)
        || crate::menu_button_release(&mut state.core, event)
        || reserved_safety(state, event, dictation, effects)
    {
        return true;
    }
    STACK
        .into_iter()
        .any(|layer| overlay_input(state, layer, event, dictation, effects))
}

fn settings_phrase(state: &mut State, event: &Event, dictation: bool, effects: &mut Vec<Effect>) -> bool {
    !state.choice_list_open()
        && !dictation
        && matches!(event, Event::Speech { .. })
        && settings_event(state, event, effects)
}

fn reserved_safety(state: &mut State, event: &Event, dictation: bool, effects: &mut Vec<Effect>) -> bool {
    if !dictation && emergency_button(state, event) {
        return system_menu(state, event, effects);
    }
    if (dictation || state.core.menu().is_none()) && password_suppresses(state, event) {
        effects.push(Effect::PasswordSpeechIgnored);
        return true;
    }
    false
}

const fn emergency_button(state: &State, event: &Event) -> bool {
    matches!(
        event,
        Event::Button {
            name: ButtonName::Guide,
            ..
        }
    ) || ((state.core.guide_held() && matches!(start_state(event), ButtonState::Pressed)
        || state.core.start_in_chord)
        && matches!(
            event,
            Event::Button {
                name: ButtonName::Start,
                ..
            }
        ))
}

fn overlay_input(
    state: &mut State,
    layer: Overlay,
    event: &Event,
    dictation: bool,
    effects: &mut Vec<Effect>,
) -> bool {
    match layer {
        Overlay::ChoiceList => {
            if state
                .launcher
                .as_ref()
                .is_some_and(|launcher| launcher.list().is_open())
            {
                !matches!(event, Event::Tick { .. }) && launcher_event(state, event, effects)
            } else {
                choice_list_input(state, event)
            }
        }
        Overlay::Settings => settings_input_layer(state, event, dictation, effects),
        Overlay::SystemMenu => !dictation && system_menu(state, event, effects),
        Overlay::Secret => secret_input(state, event),
        Overlay::Keyboard => keyboard(state, event, effects),
        Overlay::Mouse => mouse_event(state, event, effects),
        Overlay::Application => desktop_input(state, event, effects),
        Overlay::Cursor
        | Overlay::SecretUnavailable
        | Overlay::Notice
        | Overlay::MouseNotice
        | Overlay::Bar => false,
    }
}

fn desktop_favourites(state: &State) -> Vec<&String> {
    let Some(global) = state
        .core
        .contexts()
        .iter()
        .find(|context| context.app_id() == "*")
    else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for entry in global.favourites() {
        if global
            .launch_phrases()
            .iter()
            .any(|(desktop_entry, _)| desktop_entry == entry)
            && !entries.contains(&entry)
        {
            entries.push(entry);
        }
    }
    entries
}

fn desktop_input(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    let Event::Button {
        name,
        state: ButtonState::Pressed,
    } = event
    else {
        return false;
    };
    if state.core.push_to_talk(*name) {
        return false;
    }
    let Some(index) = state.desktop_focus() else {
        return false;
    };
    let entries = desktop_favourites(state);
    let next = match name {
        ButtonName::DpadLeft | ButtonName::DpadUp => index.saturating_sub(1),
        ButtonName::DpadRight | ButtonName::DpadDown => {
            index.saturating_add(1).min(entries.len().saturating_sub(1))
        }
        name if *name == state.core.pad_family().button(crate::pad::Role::Confirm) => {
            if let Some(entry) = entries.get(index)
                && let Ok(desktop_entry) = tehuti_protocol::Text::new((*entry).clone())
            {
                effects.push(Effect::Apply(Action::Launch { desktop_entry }));
            }
            index
        }
        ButtonName::South
        | ButtonName::East
        | ButtonName::North
        | ButtonName::West
        | ButtonName::LeftBumper
        | ButtonName::RightBumper
        | ButtonName::LeftTrigger
        | ButtonName::RightTrigger
        | ButtonName::Select
        | ButtonName::Record
        | ButtonName::Start
        | ButtonName::Guide
        | ButtonName::LeftStick
        | ButtonName::RightStick => return false,
    };
    state.desktop_selection = next;
    if !state.desktop_buttons.contains(name) {
        state.desktop_buttons.push(*name);
    }
    true
}

fn launcher_event(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    let family = state.core.pad_family();
    let Some(launcher) = state
        .launcher
        .as_mut()
        .filter(|launcher| launcher.list().is_open())
    else {
        return false;
    };
    match event {
        Event::Tick { elapsed } => {
            if let Some(index) = launcher.tick(*elapsed) {
                effects.push(Effect::ChooseLauncher(index));
            }
            false
        }
        Event::Speech { text } => {
            launcher.stop_timer();
            let request = crate::unit_verbs::launcher_request(text.as_str()).unwrap_or_else(|| {
                crate::unit_verbs::LauncherRequest {
                    kind: crate::unit_verbs::DestinationKind::Application,
                    query: text.as_str().into(),
                }
            });
            effects.push(Effect::OpenLauncher(request));
            true
        }
        Event::Button { name, state: button } => {
            if state.core.push_to_talk(*name) {
                launcher.stop_timer();
                return false;
            }
            let own = [
                family.button(crate::pad::Role::Confirm),
                family.button(crate::pad::Role::Back),
                ButtonName::DpadUp,
                ButtonName::DpadDown,
            ];
            if launcher.is_invitation() && !own.contains(name) {
                // The invitation leaves menus, mouse mode and shortcuts working.
                return false;
            }
            if *button == ButtonState::Pressed && !state.choice_buttons.contains(name) {
                state.choice_buttons.push(*name);
                if *name == family.button(crate::pad::Role::Confirm) {
                    if let Some(index) = launcher.confirm() {
                        effects.push(Effect::ChooseLauncher(index));
                    }
                } else if *name == family.button(crate::pad::Role::Back) {
                    launcher.cancel();
                } else if *name == ButtonName::DpadUp {
                    launcher.move_selection(false);
                } else if *name == ButtonName::DpadDown {
                    launcher.move_selection(true);
                }
            }
            true
        }
        Event::Focus { .. } => false,
    }
}

fn choice_list_input(state: &mut State, event: &Event) -> bool {
    let Event::Button { name, state: button } = event else {
        return state.choice_list_open() && matches!(event, Event::Speech { .. });
    };
    if !state.choice_list_open() {
        return false;
    }
    if *button == ButtonState::Pressed && !state.choice_buttons.contains(name) {
        state.choice_buttons.push(*name);
        let family = state.core.pad_family();
        if let Some(list) = &mut state.choice_list {
            if *name == family.button(crate::pad::Role::Back) {
                list.cancel();
            } else if *name == family.button(crate::pad::Role::Confirm) || *name == ButtonName::Start {
                list.confirm();
            } else if *name == ButtonName::DpadUp {
                list.move_selection(false);
            } else if *name == ButtonName::DpadDown {
                list.move_selection(true);
            }
        }
    }
    true
}

fn secret_input(state: &mut State, event: &Event) -> bool {
    if state.secret_prompt != SecretPrompt::Visible || lower_view_owns_pad(state) {
        return false;
    }
    let Event::Button { name, state: button } = event else {
        return false;
    };
    if *name != state.core.pad_family().button(crate::pad::Role::Confirm) || state.core.push_to_talk(*name) {
        return false;
    }
    queue_secret_confirmation(state, *button, event.clone());
    true
}

fn lower_view_owns_pad(state: &State) -> bool {
    state.choice_list_open() || state.settings.open || state.keyboard.is_open() || state.mouse.active()
}

fn queue_secret_confirmation(state: &mut State, button: ButtonState, event: Event) {
    if let Some(frame) = state.frames.last_mut() {
        frame.tasks.push_front(Task::ConfirmSecret(button, event));
        frame
            .tasks
            .push_front(Task::ConfirmPressed(button == ButtonState::Pressed));
    }
}

fn settings_input_layer(
    state: &mut State,
    event: &Event,
    dictation: bool,
    effects: &mut Vec<Effect>,
) -> bool {
    if dictation || state.core.menu().is_some() {
        return false;
    }
    settings_event(state, event, effects)
}

fn password_suppresses(state: &State, event: &Event) -> bool {
    state.keyboard.password_field_active()
        && (matches!(event, Event::Button { name, .. } if state.core.push_to_talk(*name))
            || matches!(event, Event::Speech { .. }))
}

fn mouse_event(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if !state.mouse.button(event, effects) {
        return false;
    }
    if effects
        .iter()
        .any(|effect| matches!(effect, Effect::MouseMode(_)))
        && let Some(frame) = state.frames.last_mut()
    {
        frame.tasks.push_front(Task::Speech(SpeechInput::Cancel));
    }
    if let Some(actions) = crate::escape_chord(&mut state.core, event) {
        effects.extend(actions.into_iter().map(Effect::Apply));
    }
    true
}

fn passthrough_controller(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
    if crate::menu_button_release(&mut state.core, &event) || system_menu(state, &event, effects) {
        return;
    }
    if state.keyboard.password_field_active() && matches!(event, Event::Speech { .. }) {
        effects.push(Effect::PasswordSpeechIgnored);
        return;
    }
    if let Event::Button { .. } = &event {
        let held = passthrough_talk_held(state);
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

fn passthrough_talk_held(state: &State) -> bool {
    state.core.passthrough_push_to_talk() != [ButtonName::Select]
        && state
            .core
            .passthrough_push_to_talk()
            .iter()
            .all(|member| state.passthrough_buttons.contains(member))
}

fn controller(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
    if let Event::Button { name, state: button } = &event {
        let was_held = state.passthrough_buttons.contains(name);
        state.passthrough_buttons.retain(|held| held != name);
        if *button == ButtonState::Pressed {
            state.passthrough_buttons.push(*name);
        }
        if state.core.passthrough_context() && takeback_button(state, *name, *button, was_held, effects) {
            return;
        }
    }
    routed_controller(state, event, effects);
}

fn takeback_button(
    state: &mut State,
    name: ButtonName,
    button: ButtonState,
    was_held: bool,
    effects: &mut Vec<Effect>,
) -> bool {
    if name != ButtonName::Select {
        if matches!(state.takeback, Takeback::Holding(_)) {
            state.takeback = Takeback::Idle;
        }
        return false;
    }
    if button == ButtonState::Released {
        return takeback_release(state, effects);
    }
    if !was_held && state.passthrough_buttons.len() == 1 {
        state.takeback = Takeback::Holding(Duration::ZERO);
    }
    !matches!(state.takeback, Takeback::Idle)
}

fn takeback_release(state: &mut State, effects: &mut Vec<Effect>) -> bool {
    match core::mem::take(&mut state.takeback) {
        Takeback::Fired => true,
        Takeback::Holding(_) if state.core.passthrough() => {
            passthrough_view_talk(state);
            true
        }
        Takeback::Holding(_) => {
            routed_controller(
                state,
                Event::Button {
                    name: ButtonName::Select,
                    state: ButtonState::Pressed,
                },
                effects,
            );
            false
        }
        Takeback::Idle => false,
    }
}

fn passthrough_view_talk(state: &mut State) {
    if state.core.passthrough_push_to_talk() == [ButtonName::Select]
        && !state.keyboard.password_field_active()
        && let Some(frame) = state.frames.last_mut()
    {
        frame.tasks.push_front(Task::Speech(SpeechInput::Release));
        frame.tasks.push_front(Task::Speech(SpeechInput::Press));
    }
}

fn routed_controller(state: &mut State, event: Event, effects: &mut Vec<Effect>) {
    // Released here, before any layer that consumes the release, so a held key never sticks.
    if let Event::Button {
        name,
        state: ButtonState::Released,
    } = event
        && let Some(release) = state.core.release_held_button(name)
    {
        effects.push(Effect::Apply(release));
    }
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
    match event {
        Event::Button { name, state: button } if state.core.push_to_talk(name) => {
            frame.tasks.push_front(Task::Speech(match button {
                ButtonState::Pressed => SpeechInput::Press,
                ButtonState::Released => SpeechInput::Release,
            }));
        }
        Event::Button { name, state: button }
            if name == state.core.pad_family().button(crate::pad::Role::Confirm) =>
        {
            frame.tasks.push_front(Task::ConfirmSecret(button, event));
            frame
                .tasks
                .push_front(Task::ConfirmPressed(button == ButtonState::Pressed));
        }
        Event::Button { .. } | Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => {
            frame.tasks.push_front(Task::Route(event));
        }
    }
}

/// The open keyboard takes speech as dictation and most buttons; "keyboard" opens it.
fn keyboard(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if keyboard_opening(state, event) {
        return true;
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
            keyboard_start(state, event, effects)
        }
        Event::Button { name, state: button } => {
            let shoulders_move_caret = state.core.level() == crate::Level::App;
            let response = state.keyboard.button(*name, *button, shoulders_move_caret);
            if response == crate::keyboard::Response::StopTalking {
                if let Some(frame) = state.frames.last_mut() {
                    frame.tasks.push_front(Task::Speech(SpeechInput::Release));
                }
                return true;
            }
            send_response(response, effects)
        }
        Event::Speech { .. } | Event::Focus { .. } | Event::Tick { .. } => false,
    }
}

fn keyboard_start(state: &mut State, event: &Event, effects: &mut Vec<Effect>) -> bool {
    if let Some(actions) = crate::escape_chord(&mut state.core, event) {
        effects.extend(actions.into_iter().map(Effect::Apply));
    }
    let guide_held = state.core.guide_held();
    let response = state.keyboard.button(ButtonName::Start, start_state(event), true);
    if guide_held {
        state.keyboard.cancel_send();
    } else if start_state(event) == ButtonState::Pressed && state.core.push_to_talk(ButtonName::Start) {
        state.keyboard.arm_start_dictation();
    }
    if response == crate::keyboard::Response::StopTalking
        && let Some(frame) = state.frames.last_mut()
    {
        frame.tasks.push_front(Task::Speech(SpeechInput::Release));
    }
    send_response(response, effects)
}

fn keyboard_opening(state: &mut State, event: &Event) -> bool {
    let Event::Button { name, state: button } = event else {
        return false;
    };
    if state.opening_button == Some(*name) {
        if *button == ButtonState::Released {
            state.opening_button = None;
        }
        return true;
    }
    if configured_keyboard_press(state, *name, *button) {
        state.opening_button = Some(*name);
        state.keyboard.open();
        return true;
    }
    false
}

fn configured_keyboard_press(state: &State, name: ButtonName, button: ButtonState) -> bool {
    let (mode, opening) = state.core.keyboard_config();
    !state.keyboard.is_open()
        && (state.text_field || state.keyboard.terminal() || mode == tehuti_protocol::OnTextField::Open)
        && mode != tehuti_protocol::OnTextField::Off
        && name == opening
        && button == ButtonState::Pressed
}

const fn start_state(event: &Event) -> ButtonState {
    match event {
        Event::Button { state, .. } => *state,
        Event::Focus { .. } | Event::Tick { .. } | Event::Speech { .. } => ButtonState::Released,
    }
}

/// True when the keyboard consumed the button; a send becomes a text action.
fn send_response(response: crate::keyboard::Response, effects: &mut Vec<Effect>) -> bool {
    match response {
        crate::keyboard::Response::Pass => false,
        crate::keyboard::Response::Consumed | crate::keyboard::Response::StopTalking => true,
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
