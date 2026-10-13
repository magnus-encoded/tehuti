//! Smithay state and protocol handlers. Shows one toplevel fullscreen and gives it keyboard focus.

use std::time::Instant;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{Space, Window},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, KeyboardHandle, KeyboardTarget, Keycode, ModifiersState, XkbConfig, xkb},
        pointer::CursorImageStatus,
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_surface::WlSurface},
        },
    },
    utils::{SERIAL_COUNTER, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
        },
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        shm::{ShmHandler, ShmState},
    },
};
use tehuti_protocol::{Action, Event, KeyName};

struct RedactedSecretProvider(Box<dyn tehuti_core::secret::SecretProvider>);

impl core::fmt::Debug for RedactedSecretProvider {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SecretProvider(<redacted>)")
    }
}

#[derive(Debug, Default)]
struct SecretConfirmationState {
    frame_submitted: bool,
    armed: bool,
    south_pressed: bool,
}

#[derive(Debug)]
struct SecretLookupResult {
    provider: RedactedSecretProvider,
    authorization: tehuti_core::secret::AuthorizedSecretLookup,
    value: Result<tehuti_core::secret::SecretValue, tehuti_core::secret::SecretProviderUnavailable>,
}

type SecretLookupReceiver = std::sync::mpsc::Receiver<SecretLookupResult>;

/// XKB keycodes are evdev codes plus 8.
const XKB_KEYCODE_OFFSET: u32 = 8;

#[derive(Debug)]
pub struct Compositor {
    pub start_time: Instant,
    pub space: Space<Window>,
    pub output: Option<Output>,
    core: tehuti_core::State,
    previous_tick: Instant,
    plugins: Option<crate::plugins::Plugins>,
    quit_requested: bool,
    speech_state: tehuti_core::speech::SpeechState,
    speech_process: Option<crate::speech::SpeechProcess>,
    secret_gate: tehuti_core::secret::SecretConfirmationGate,
    secret_provider: Option<RedactedSecretProvider>,
    secret_lookup: Option<SecretLookupReceiver>,
    secret_focus_generation: u64,
    secret_surface_id: u64,
    secret_focus_surface: Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
    secret_confirmation_label: Option<(String, String)>,
    secret_confirmation: SecretConfirmationState,
    secret_notice_until: Option<Instant>,
    notice_until: Option<Instant>,
    notice_text: Option<String>,
    launchers: Vec<std::process::Child>,
    desktop_entries: crate::desktop::EntryCache,
    launch_socket: Option<std::ffi::OsString>,
    windows: tehuti_core::windows::Windows<WlSurface>,
    surfaces: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    keyboard: KeyboardHandle<Self>,
    controller_session: tehuti_core::controller::State,
    seat: Seat<Self>,
    text_keys: std::collections::HashMap<char, (Keycode, ModifiersState)>,
    pub(crate) text_input: crate::text_input::TextInput,
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("keyboard keymap: {0}")]
    Keymap(#[from] smithay::input::keyboard::Error),
    #[error("keyboard keymap compilation failed")]
    KeymapCompilation,
    #[error("desktop entry refresh worker: {0}")]
    DesktopEntries(#[from] std::io::Error),
}

impl Compositor {
    /// Creates the Wayland globals and a seat with one keyboard.
    ///
    /// # Errors
    /// The keyboard keymap fails to load or the desktop entry worker cannot start.
    pub fn new(display: &DisplayHandle) -> Result<Self, SetupError> {
        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(display, "tehuti");
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap =
            xkb::Keymap::new_from_names(&context, "", "", "", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
                .ok_or(SetupError::KeymapCompilation)?;
        let keyboard = seat.add_keyboard(XkbConfig::default(), 200, 25)?;
        let text_keys = text_keys(&keymap);
        let mut compositor = Self {
            start_time: Instant::now(),
            space: Space::default(),
            output: None,
            core: tehuti_core::State::new(),
            previous_tick: Instant::now(),
            plugins: None,
            quit_requested: false,
            speech_state: tehuti_core::speech::SpeechState::default(),
            speech_process: None,
            secret_gate: tehuti_core::secret::SecretConfirmationGate::new(),
            secret_provider: Some(RedactedSecretProvider(Box::new(
                tehuti_core::secret::DenySecretProvider,
            ))),
            secret_lookup: None,
            secret_focus_generation: 0,
            secret_surface_id: 0,
            secret_focus_surface: None,
            secret_confirmation_label: None,
            secret_confirmation: SecretConfirmationState::default(),
            secret_notice_until: None,
            notice_until: None,
            notice_text: None,
            launchers: Vec::new(),
            desktop_entries: crate::desktop::EntryCache::new()?,
            launch_socket: None,
            windows: tehuti_core::windows::Windows::new(),
            surfaces: CompositorState::new::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            shm_state: ShmState::new::<Self>(display, vec![]),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(display),
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(display),
            keyboard,
            controller_session: tehuti_core::controller::State::default(),
            seat,
            text_keys,
            text_input: crate::text_input::TextInput::new(display),
        };
        compositor
            .keyboard
            .clone()
            .set_keymap_from_string(&mut compositor, keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))?;
        Ok(compositor)
    }

    /// Installs contexts before accepting clients.
    pub fn set_contexts(&mut self, contexts: Vec<tehuti_protocol::Context>) {
        self.core = tehuti_core::State::with_contexts(contexts);
    }

    #[must_use]
    pub fn initialized_plugins(&self) -> usize {
        self.plugins
            .as_ref()
            .map_or(0, crate::plugins::Plugins::initialized_count)
    }

    /// Plugin decisions the core still holds a fallback for.
    #[must_use]
    pub const fn pending_plugin_decisions(&self) -> usize {
        self.core.pending_decisions()
    }

    pub fn set_plugins(&mut self, plugins: crate::plugins::Plugins) {
        self.plugins = Some(plugins);
    }

    pub fn set_secret_provider(&mut self, provider: Box<dyn tehuti_core::secret::SecretProvider>) {
        self.secret_provider = Some(RedactedSecretProvider(provider));
    }

    pub(crate) fn apply_plugin_action(&mut self, action: &Action) {
        if !self.quit_requested {
            self.apply(action);
        }
    }

    pub(crate) fn plugin_reply(
        &mut self,
        key: tehuti_core::plugin::RequestKey,
        result: Result<Vec<Action>, String>,
    ) -> Option<tehuti_core::plugin::Rejection> {
        let (next, actions, rejection) = tehuti_core::plugin::step(
            core::mem::take(&mut self.core),
            tehuti_core::plugin::Input::Reply { key, result },
        );
        self.core = next;
        for action in &actions {
            self.apply_plugin_action(action);
        }
        rejection
    }

    pub fn set_speech(&mut self, process: crate::speech::SpeechProcess) {
        self.speech_process = Some(process);
    }

    #[must_use]
    pub fn speech_ready(&self) -> bool {
        self.speech_process
            .as_ref()
            .is_some_and(crate::speech::SpeechProcess::is_ready)
    }

    #[must_use]
    pub fn speech_notice_visible(&self) -> bool {
        self.notice_until.is_some_and(|until| Instant::now() < until)
    }

    #[must_use]
    pub fn visible_notice_text(&self) -> Option<String> {
        self.secret_confirmation_text()
            .or_else(|| {
                self.secret_notice_until
                    .is_some_and(|until| Instant::now() < until)
                    .then(|| "SECRET DELIVERY UNAVAILABLE".into())
            })
            .or_else(|| {
                self.speech_notice_visible().then(|| {
                    self.notice_text
                        .clone()
                        .unwrap_or_else(|| "NO SPEECH MAPPING".into())
                })
            })
    }

    pub fn speech_input(&mut self, input: tehuti_core::speech::SpeechInput) {
        use tehuti_core::speech::{SpeechEffect, speech_step};
        if let tehuti_core::speech::SpeechInput::Transcript { session, text } = &input {
            tracing::info!(session, text = text.as_str(), "speech transcript received");
        }
        let transcript = matches!(input, tehuti_core::speech::SpeechInput::Transcript { .. });
        let (state, effects) = speech_step(core::mem::take(&mut self.speech_state), input);
        self.speech_state = state;
        if transcript && effects.is_empty() {
            tracing::info!("speech transcript dropped for a stale session");
        }
        for effect in effects {
            match &effect {
                SpeechEffect::Start { session } => tracing::info!(session, "speech session started"),
                SpeechEffect::Stop { session, transcribe } => {
                    tracing::info!(session, transcribe, "speech session stopped");
                }
                SpeechEffect::Speech { .. } => {}
                SpeechEffect::Failed { session } => tracing::warn!(session, "speech transcription failed"),
            }
            if let SpeechEffect::Speech { text, .. } = effect {
                self.handle(&Event::Speech { text });
            } else if let SpeechEffect::Failed { .. } = effect {
                self.show_notice("SPEECH FAILED");
            } else if let Some(process) = &mut self.speech_process
                && let Err(error) = process.send(&effect)
            {
                tracing::warn!(%error, "speech command failed");
                self.speech_input(tehuti_core::speech::SpeechInput::Unavailable);
            }
        }
    }

    pub(crate) fn plugin_exited(&mut self, plugin: usize) {
        let (next, actions, _) = tehuti_core::plugin::step(
            core::mem::take(&mut self.core),
            tehuti_core::plugin::Input::Exited { plugin },
        );
        self.core = next;
        for action in &actions {
            self.apply_plugin_action(action);
        }
    }

    pub(crate) fn supervise_plugins(&mut self) {
        let exited = self
            .plugins
            .as_mut()
            .map_or_else(Vec::new, crate::plugins::Plugins::supervise);
        for plugin in exited {
            self.plugin_exited(plugin);
        }
        self.advance_time();
    }

    /// Supplies elapsed wall time to the pure core before input or replies.
    pub fn advance_time(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.previous_tick);
        self.previous_tick = now;
        self.handle(&Event::Tick { elapsed });
    }

    pub fn controller_input(&mut self, input: tehuti_core::controller::Input) {
        let (next, events) =
            tehuti_core::controller::step(core::mem::take(&mut self.controller_session), input);
        self.controller_session = next;
        for event in events {
            self.handle(&event);
        }
    }

    /// Runs one event through the core step and carries out the resulting actions.
    pub fn handle(&mut self, event: &Event) {
        use tehuti_core::speech::SpeechInput;
        if matches!(event, Event::Tick { .. }) {
            self.previous_tick = Instant::now();
            self.route_event(event);
        } else if !matches!(event, Event::Focus { .. }) {
            self.advance_time();
        }
        let exited = self
            .plugins
            .as_mut()
            .map_or_else(Vec::new, crate::plugins::Plugins::supervise);
        for plugin in exited {
            self.plugin_exited(plugin);
        }
        if let Event::Button {
            name: tehuti_protocol::ButtonName::South,
            state,
        } = event
        {
            self.secret_confirmation.south_pressed = *state == tehuti_protocol::ButtonState::Pressed;
        }
        if matches!(event, Event::Focus { .. }) {
            self.reset_secret_confirmation_for_focus();
        }
        match event {
            Event::Button {
                name: tehuti_protocol::ButtonName::Record,
                state,
            } => {
                self.speech_input(match state {
                    tehuti_protocol::ButtonState::Pressed => SpeechInput::Press,
                    tehuti_protocol::ButtonState::Released => SpeechInput::Release,
                });
                return;
            }
            Event::Button {
                name: tehuti_protocol::ButtonName::South,
                state,
            } if self.handle_secret_confirmation_button(*state) => return,
            Event::Focus { .. } => self.speech_input(SpeechInput::Cancel),
            Event::Tick { .. } | Event::Button { .. } | Event::Speech { .. } => {}
        }
        if !matches!(event, Event::Tick { .. }) {
            self.route_event(event);
        }
        if matches!(event, Event::Focus { .. }) {
            self.advance_time();
        }
        if let Event::Tick { elapsed } = event {
            self.poll_tick(*elapsed);
        }
    }

    fn route_event(&mut self, event: &Event) {
        let active = self.plugins.as_mut().is_some_and(|plugins| plugins.active(event));
        let (mut next, mut actions, route_plugin) =
            tehuti_core::step_with_plugin(core::mem::take(&mut self.core), event, active);
        let request = self
            .plugins
            .as_mut()
            .and_then(|plugins| plugins.dispatch(event, route_plugin));
        if route_plugin {
            if let Some((key, permissions)) = request {
                let (state, mut routed, _) = tehuti_core::plugin::step(
                    next,
                    tehuti_core::plugin::Input::Request {
                        key,
                        event: event.clone(),
                        permissions,
                    },
                );
                next = state;
                actions.append(&mut routed);
            } else {
                let (state, mut fallback) = tehuti_core::step(next, event);
                next = state;
                actions.append(&mut fallback);
            }
        }
        self.core = next;
        if let Event::Speech { text } = event {
            match actions.first() {
                Some(Action::Notice { text: notice }) if notice.as_str() == "NO SPEECH MAPPING" => {
                    tracing::info!(text = text.as_str(), "speech has no mapping");
                }
                Some(action) => tracing::info!(text = text.as_str(), ?action, "speech mapped"),
                None if route_plugin => tracing::info!(text = text.as_str(), "speech routed to a plugin"),
                None => tracing::info!(text = text.as_str(), "speech produced no action"),
            }
        }
        for action in &actions {
            self.apply(action);
        }
    }

    fn poll_tick(&mut self, elapsed: core::time::Duration) {
        let inputs = self
            .speech_process
            .as_mut()
            .map_or_else(Vec::new, |process| process.poll(elapsed));
        for input in inputs {
            self.speech_input(input);
        }
        self.poll_secret_lookup();
        self.poll_launchers();
        self.desktop_entries.poll();
    }

    fn reset_secret_confirmation_for_focus(&mut self) {
        self.secret_focus_generation = self.secret_focus_generation.wrapping_add(1);
        self.secret_gate.cancel();
        self.secret_confirmation_label = None;
        self.secret_confirmation.frame_submitted = false;
        self.secret_confirmation.armed = false;
        let focus = self
            .keyboard
            .current_focus()
            .filter(smithay::reexports::wayland_server::Resource::is_alive);
        if self.secret_focus_surface != focus {
            self.secret_surface_id = self.secret_surface_id.wrapping_add(1);
            self.secret_focus_surface = focus;
        }
    }

    #[must_use]
    pub const fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Requests a one-use controller confirmation for the focused surface.
    pub fn request_secret_delivery(&mut self, reference: tehuti_protocol::SecretReference) -> bool {
        if self.secret_lookup.is_some() || self.secret_provider.is_none() {
            return false;
        }
        let Some(focus) = self
            .keyboard
            .current_focus()
            .filter(smithay::reexports::wayland_server::Resource::is_alive)
        else {
            return false;
        };
        let target = smithay::wayland::compositor::with_states(&focus, |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .map(|attributes| {
                    let attributes = attributes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    attributes
                        .title
                        .as_ref()
                        .filter(|title| !title.trim().is_empty())
                        .cloned()
                        .or_else(|| attributes.app_id.clone())
                        .unwrap_or_else(|| "unknown window".into())
                })
        })
        .unwrap_or_else(|| "unknown window".into());
        let Some(destination) = self.current_secret_destination() else {
            return false;
        };
        let entry_label = sanitize_confirmation_label(&reference.entry, 21);
        let requested = self
            .secret_gate
            .request(
                reference,
                destination,
                self.start_time.elapsed(),
                core::time::Duration::from_secs(20),
            )
            .is_some();
        if requested {
            self.secret_confirmation_label = Some((entry_label, sanitize_confirmation_label(&target, 29)));
            self.secret_confirmation.frame_submitted = false;
            self.secret_confirmation.armed = false;
        }
        requested
    }

    #[must_use]
    pub fn secret_confirmation_text(&self) -> Option<String> {
        self.secret_confirmation_label
            .as_ref()
            .and_then(|(entry, target)| {
                let destination = self.current_secret_destination()?;
                let _pending = self.secret_gate.pending(self.start_time.elapsed(), destination)?;
                Some(format!("CONFIRM SECRET: {entry}\nTARGET: {target}"))
            })
    }

    /// Records that a damaged frame containing the confirmation was submitted.
    pub fn note_secret_confirmation_frame_submitted(&mut self) {
        if self.secret_confirmation_text().is_some() {
            self.secret_confirmation.frame_submitted = true;
            self.secret_confirmation.armed = !self.secret_confirmation.south_pressed;
        }
    }

    fn current_secret_destination(&self) -> Option<tehuti_core::secret::SecretDestination> {
        let focus = self
            .keyboard
            .current_focus()
            .filter(smithay::reexports::wayland_server::Resource::is_alive)?;
        if self.secret_focus_surface.as_ref() != Some(&focus) {
            return None;
        }
        Some(tehuti_core::secret::SecretDestination {
            surface: self.secret_surface_id,
            focus_generation: self.secret_focus_generation,
        })
    }

    fn handle_secret_confirmation_button(&mut self, state: tehuti_protocol::ButtonState) -> bool {
        let Some(destination) = self.current_secret_destination() else {
            return false;
        };
        if self
            .secret_gate
            .pending(self.start_time.elapsed(), destination)
            .is_none()
        {
            return false;
        }
        match state {
            tehuti_protocol::ButtonState::Released => {
                self.secret_confirmation.south_pressed = false;
                if self.secret_confirmation.frame_submitted {
                    self.secret_confirmation.armed = true;
                }
            }
            tehuti_protocol::ButtonState::Pressed if self.secret_confirmation.armed => {
                self.secret_confirmation.south_pressed = true;
                self.secret_confirmation.armed = false;
                let _consumed = self.confirm_pending_secret();
            }
            tehuti_protocol::ButtonState::Pressed => self.secret_confirmation.south_pressed = true,
        }
        true
    }

    fn confirm_pending_secret(&mut self) -> bool {
        let Some(destination) = self.current_secret_destination() else {
            return false;
        };
        let now = self.start_time.elapsed();
        let Some(confirmation_id) = self
            .secret_gate
            .pending(now, destination)
            .map(tehuti_core::secret::PendingSecretDelivery::confirmation_id)
        else {
            return false;
        };
        let Some(authorization) = self.secret_gate.confirm(confirmation_id, now, destination) else {
            return true;
        };
        self.secret_confirmation_label = None;
        let Some(mut provider) = self.secret_provider.take() else {
            self.secret_notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
            return true;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("tehuti-secret-lookup".into())
            .spawn(move || {
                let value = provider.0.lookup(&authorization.reference);
                let _sent = sender.send(SecretLookupResult {
                    provider,
                    authorization,
                    value,
                });
            });
        if spawned.is_err() {
            self.secret_notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
            return true;
        }
        self.secret_lookup = Some(receiver);
        true
    }

    fn poll_secret_lookup(&mut self) {
        let Some(receiver) = &self.secret_lookup else {
            return;
        };
        let lookup_result = match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.secret_lookup = None;
                self.secret_notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
                return;
            }
        };
        let Some(lookup) = lookup_result else {
            return;
        };
        self.secret_lookup = None;
        self.secret_provider = Some(lookup.provider);
        let Some(secret) = lookup.value.ok() else {
            self.secret_notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
            return;
        };
        if self.current_secret_destination() != Some(lookup.authorization.destination) {
            return;
        }
        let Some(focus) = self
            .keyboard
            .current_focus()
            .filter(smithay::reexports::wayland_server::Resource::is_alive)
        else {
            return;
        };
        let keymap = self.text_keys.clone();
        let seat = self.seat.clone();
        let delivered = crate::secret_delivery::deliver_if_current(
            lookup.authorization.destination,
            self.current_secret_destination(),
            &secret,
            &keymap,
            |(keycode, modifiers)| {
                focus.modifiers(&seat, self, modifiers, SERIAL_COUNTER.next_serial());
                self.tap_keycode(keycode);
                focus.modifiers(
                    &seat,
                    self,
                    ModifiersState::default(),
                    SERIAL_COUNTER.next_serial(),
                );
            },
        );
        if !delivered {
            self.secret_notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
        }
    }

    /// Carries out one action from the core step.
    fn apply(&mut self, action: &Action) {
        match action {
            Action::Key { key } => self.tap(*key),
            Action::Text { text } => {
                if !self.text_input.send(text.as_str()) {
                    self.type_text(text.as_str());
                }
            }
            Action::SecretRequest { reference } => {
                if !self.request_secret_delivery(reference.clone()) {
                    self.secret_notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
                }
            }
            Action::Quit => {
                self.speech_input(tehuti_core::speech::SpeechInput::Cancel);
                self.quit_requested = true;
            }
            Action::Launch { desktop_entry } => self.launch_application(desktop_entry),
            Action::Close => {
                if let Some(tehuti_core::windows::Effect::Close(surface)) =
                    self.windows.step(tehuti_core::windows::Input::Close)
                {
                    if let Some(window) = self.window_for_root(&surface).cloned() {
                        if let Some(toplevel) = window.toplevel() {
                            toplevel.send_close();
                        }
                        self.space.unmap_elem(&window);
                    }
                    self.sync_window_focus();
                }
            }
            Action::Notice { text } => {
                self.show_notice(text.as_str());
            }
        }
    }

    fn show_notice(&mut self, text: &str) {
        let _effect = self.windows.step(tehuti_core::windows::Input::Notify);
        self.notice_text = Some(text.into());
        self.notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
    }

    fn tap(&mut self, key: KeyName) {
        let evdev_code = key.evdev_code();
        let keycode = Keycode::new(evdev_code.saturating_add(XKB_KEYCODE_OFFSET));
        self.tap_keycode(keycode);
    }

    fn tap_keycode(&mut self, keycode: Keycode) {
        let keyboard = self.keyboard.clone();
        for state in tehuti_core::controller::key_tap() {
            let state = match state {
                tehuti_protocol::ButtonState::Pressed => smithay::backend::input::KeyState::Pressed,
                tehuti_protocol::ButtonState::Released => smithay::backend::input::KeyState::Released,
            };
            let time = self.millis_since_start();
            let _forwarded: Option<()> = keyboard.input(
                self,
                keycode,
                state,
                SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| FilterResult::Forward,
            );
        }
    }

    fn type_text(&mut self, text: &str) {
        let Some(focus) = self
            .keyboard
            .current_focus()
            .filter(smithay::reexports::wayland_server::Resource::is_alive)
        else {
            tracing::warn!("text action skipped: no focused client");
            return;
        };
        let client = smithay::wayland::compositor::with_states(&focus, |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|attributes| {
                    attributes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .app_id
                        .clone()
                })
        });
        let surface = smithay::reexports::wayland_server::Resource::id(&focus);
        let seat = self.seat.clone();
        for character in text.chars() {
            if character == '\n' {
                self.tap(KeyName::Enter);
                continue;
            }
            let Some((keycode, modifiers)) = self.text_keys.get(&character).copied() else {
                tracing::warn!(
                    ?client,
                    ?surface,
                    ?character,
                    "text character unavailable in client keymap; skipped"
                );
                continue;
            };
            focus.modifiers(&seat, self, modifiers, SERIAL_COUNTER.next_serial());
            self.tap_keycode(keycode);
            focus.modifiers(
                &seat,
                self,
                ModifiersState::default(),
                SERIAL_COUNTER.next_serial(),
            );
        }
    }

    fn millis_since_start(&self) -> u32 {
        u32::try_from(self.start_time.elapsed().as_millis()).unwrap_or(u32::MAX)
    }

    /// Sizes every window to the output and marks it fullscreen.
    pub fn fill_output(&self) {
        let Some(mode) = self.output.as_ref().and_then(Output::current_mode) else {
            return;
        };
        for window in self.space.elements() {
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|pending| {
                    pending.size = Some(mode.size.to_logical(1));
                    let _newly_set = pending.states.set(xdg_toplevel::State::Fullscreen);
                });
                if toplevel.is_initial_configure_sent() {
                    let _serial = toplevel.send_pending_configure();
                }
            }
        }
    }

    fn launch_application(&mut self, desktop_entry: &tehuti_protocol::Text) {
        let installed = self.desktop_entries.entries().to_vec();
        let decision = self.windows.step(tehuti_core::windows::Input::Launch {
            desktop_entry: desktop_entry.as_str().into(),
            installed,
        });
        if decision != Some(tehuti_core::windows::Effect::Launch) {
            self.show_launch_failure();
            return;
        }
        let result = self
            .launch_socket
            .as_ref()
            .ok_or_else(|| "no compositor socket".to_owned())
            .and_then(|socket| {
                if self.launchers.len() >= 16 {
                    return Err("too many pending launches".into());
                }
                crate::desktop::launch(desktop_entry.as_str(), socket)
            });
        match result {
            Ok(child) => self.launchers.push(child),
            Err(error) => {
                tracing::warn!(%error, "desktop launch failed");
                self.show_launch_failure();
            }
        }
    }

    pub fn set_launch_socket(&mut self, socket: std::ffi::OsString) {
        self.launch_socket = Some(socket);
    }

    fn show_launch_failure(&mut self) {
        self.notice_text = Some("APPLICATION UNAVAILABLE".into());
        self.notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
    }

    fn poll_launchers(&mut self) {
        let mut failed = false;
        self.launchers.retain_mut(|child| match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                failed |= !status.success();
                false
            }
            Err(error) => {
                tracing::warn!(%error, "desktop launcher wait failed");
                failed = true;
                let _killed = child.kill();
                let _reaped = child.wait();
                false
            }
        });
        if failed {
            self.show_launch_failure();
        }
    }

    fn sync_window_focus(&mut self) {
        let focus = self.windows.focus().cloned();
        self.keyboard
            .clone()
            .set_focus(self, focus.clone(), SERIAL_COUNTER.next_serial());
        if let Some(window) = focus
            .as_ref()
            .and_then(|surface| self.window_for_root(surface))
            .cloned()
        {
            if let Some(toplevel) = window.toplevel() {
                self.update_focus(toplevel);
            }
        } else {
            self.handle(&Event::Focus {
                app_id: None,
                title: String::new(),
            });
        }
    }

    fn update_focus(&mut self, surface: &ToplevelSurface) {
        if self.keyboard.current_focus().as_ref() != Some(surface.wl_surface()) {
            return;
        }
        let event = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
            let attributes = states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>();
            attributes.map(|attributes| {
                let attributes = attributes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Event::Focus {
                    app_id: attributes.app_id.clone(),
                    title: attributes.title.clone().unwrap_or_default(),
                }
            })
        });
        if let Some(event) = event {
            self.handle(&event);
        }
    }

    fn window_for_root(&self, root: &WlSurface) -> Option<&Window> {
        self.space.elements().find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == root)
        })
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        for child in &mut self.launchers {
            let _killed = child.kill();
            let _reaped = child.wait();
        }
    }
}

fn text_keys(keymap: &xkb::Keymap) -> std::collections::HashMap<char, (Keycode, ModifiersState)> {
    let mut keys = std::collections::HashMap::new();
    let mut state = xkb::State::new(keymap);
    for layout in 0..keymap.num_layouts() {
        for mask in 0..256 {
            let _changed = state.update_mask(mask, 0, 0, 0, 0, layout);
            let mut modifiers = ModifiersState::default();
            modifiers.update_with(&state);
            for code in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
                let keycode = Keycode::new(code);
                let text = state.key_get_utf8(keycode);
                let mut characters = text.chars();
                if let Some(character) = characters.next()
                    && characters.next().is_none()
                {
                    let _entry = keys.entry(character).or_insert((keycode, modifiers));
                }
            }
        }
    }
    keys
}

fn sanitize_confirmation_label(value: &str, maximum: usize) -> String {
    let mut label: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '?'
            }
        })
        .take(maximum)
        .collect();
    if value.chars().count() > maximum {
        label.push_str("...");
    }
    if label.is_empty() {
        label.push_str("unnamed");
    }
    label
}

#[derive(Debug, Default)]
pub struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl CompositorHandler for Compositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.surfaces
    }

    fn client_compositor_state<'client>(&self, client: &'client Client) -> &'client CompositorClientState {
        static FALLBACK: std::sync::LazyLock<CompositorClientState> =
            std::sync::LazyLock::new(CompositorClientState::default);
        client
            .get_data::<ClientState>()
            .map_or(&*FALLBACK, |data| &data.compositor_state)
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if is_sync_subsurface(surface) {
            return;
        }
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if let Some(window) = self.window_for_root(&root) {
            window.on_commit();
            if let Some(toplevel) = window.toplevel()
                && !toplevel.is_initial_configure_sent()
            {
                let _serial = toplevel.send_configure();
            }
        }
    }
}

impl BufferHandler for Compositor {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Compositor {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let _effect = self
            .windows
            .step(tehuti_core::windows::Input::Map(surface.wl_surface().clone()));
        self.space
            .map_element(Window::new_wayland_window(surface), (0, 0), true);
        self.fill_output();
        self.sync_window_focus();
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.update_focus(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.update_focus(&surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let previous_focus = self.windows.focus().cloned();
        let _effect = self
            .windows
            .step(tehuti_core::windows::Input::Unmap(surface.wl_surface().clone()));
        if let Some(window) = self.window_for_root(surface.wl_surface()).cloned() {
            self.space.unmap_elem(&window);
        }
        if self.windows.focus() != previous_focus.as_ref() {
            self.sync_window_focus();
        }
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}
}

impl SeatHandler for Compositor {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&WlSurface>) {
        self.text_input.focus(focused.cloned());
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}

impl SelectionHandler for Compositor {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Compositor {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Compositor {}
impl ServerDndGrabHandler for Compositor {}
impl OutputHandler for Compositor {}

delegate_compositor!(Compositor);
delegate_shm!(Compositor);
delegate_xdg_shell!(Compositor);
delegate_seat!(Compositor);
delegate_data_device!(Compositor);
delegate_output!(Compositor);
