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

/// XKB keycodes are evdev codes plus 8.
const XKB_KEYCODE_OFFSET: u32 = 8;

#[derive(Debug)]
pub struct Compositor {
    pub start_time: Instant,
    pub space: Space<Window>,
    pub output: Option<Output>,
    core: tehuti_core::State,
    plugins: Option<crate::plugins::Plugins>,
    quit_requested: bool,
    speech_state: tehuti_core::speech::SpeechState,
    speech_process: Option<crate::speech::SpeechProcess>,
    notice_until: Option<Instant>,
    surfaces: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    keyboard: KeyboardHandle<Self>,
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
}

impl Compositor {
    /// Creates the Wayland globals and a seat with one keyboard.
    ///
    /// # Errors
    /// The keyboard keymap fails to load.
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
            plugins: None,
            quit_requested: false,
            speech_state: tehuti_core::speech::SpeechState::default(),
            speech_process: None,
            notice_until: None,
            surfaces: CompositorState::new::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            shm_state: ShmState::new::<Self>(display, vec![]),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(display),
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(display),
            keyboard,
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

    pub fn set_plugins(&mut self, plugins: crate::plugins::Plugins) {
        self.plugins = Some(plugins);
    }

    pub(crate) fn apply_plugin_action(&mut self, action: &Action) {
        if !self.quit_requested {
            self.apply(action);
        }
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

    pub fn speech_input(&mut self, input: tehuti_core::speech::SpeechInput) {
        use tehuti_core::speech::{SpeechEffect, speech_step};
        let (state, effects) = speech_step(core::mem::take(&mut self.speech_state), input);
        self.speech_state = state;
        for effect in effects {
            if let SpeechEffect::Speech { text, .. } = effect {
                self.handle(&Event::Speech { text });
            } else if let Some(process) = &mut self.speech_process
                && let Err(error) = process.send(&effect)
            {
                tracing::warn!(%error, "speech command failed");
                self.speech_input(tehuti_core::speech::SpeechInput::Unavailable);
            }
        }
    }

    /// Runs one event through the core step and carries out the resulting actions.
    pub fn handle(&mut self, event: &Event) {
        use tehuti_core::speech::SpeechInput;
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
            Event::Focus { .. } => self.speech_input(SpeechInput::Cancel),
            Event::Tick { elapsed } => {
                let inputs = self
                    .speech_process
                    .as_mut()
                    .map_or_else(Vec::new, |process| process.poll(*elapsed));
                for input in inputs {
                    self.speech_input(input);
                }
            }
            Event::Button { .. } | Event::Speech { .. } => {}
        }
        let active = self.plugins.as_mut().is_some_and(|plugins| plugins.active(event));
        let (next, actions, route_plugin) =
            tehuti_core::step_with_plugin(core::mem::take(&mut self.core), event, active);
        if let Some(plugins) = &mut self.plugins {
            plugins.dispatch(event, route_plugin);
        }
        self.core = next;
        for action in &actions {
            self.apply(action);
        }
    }

    #[must_use]
    pub const fn quit_requested(&self) -> bool {
        self.quit_requested
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
            Action::Quit => {
                self.speech_input(tehuti_core::speech::SpeechInput::Cancel);
                self.quit_requested = true;
            }
            Action::Notice { .. } => {
                self.notice_until = Instant::now().checked_add(core::time::Duration::from_secs(3));
            }
        }
    }

    fn tap(&mut self, key: KeyName) {
        let evdev_code = key.evdev_code();
        let keycode = Keycode::new(evdev_code.saturating_add(XKB_KEYCODE_OFFSET));
        self.tap_keycode(keycode);
    }

    fn tap_keycode(&mut self, keycode: Keycode) {
        let keyboard = self.keyboard.clone();
        for state in [
            smithay::backend::input::KeyState::Pressed,
            smithay::backend::input::KeyState::Released,
        ] {
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
        let focus = surface.wl_surface().clone();
        self.space
            .map_element(Window::new_wayland_window(surface.clone()), (0, 0), true);
        self.fill_output();
        let keyboard = self.keyboard.clone();
        keyboard.set_focus(self, Some(focus), SERIAL_COUNTER.next_serial());
        self.update_focus(&surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.update_focus(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.update_focus(&surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if self.keyboard.current_focus().as_ref() == Some(surface.wl_surface()) {
            let keyboard = self.keyboard.clone();
            keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
            self.handle(&Event::Focus {
                app_id: None,
                title: String::new(),
            });
        }
        let window = self.window_for_root(surface.wl_surface()).cloned();
        if let Some(window) = window {
            self.space.unmap_elem(&window);
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
