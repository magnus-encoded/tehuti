//! Smithay state and protocol handlers for one output and its tiling layout.

use std::time::Instant;

use crate::layout_window::LayoutWindow;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_fractional_scale, delegate_output, delegate_seat,
    delegate_shm, delegate_viewporter, delegate_xdg_shell,
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
        fractional_scale::{FractionalScaleHandler, FractionalScaleManagerState, with_fractional_scale},
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
        },
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        shm::{ShmHandler, ShmState},
        viewporter::ViewporterState,
    },
};
use tehuti_core::dispatch::{Effect as DispatchEffect, Input as DispatchInput};
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
    pointer: smithay::input::pointer::PointerHandle<Self>,
    pointer_location: smithay::utils::Point<f64, smithay::utils::Logical>,
    mouse_mode: bool,
    pub space: Space<LayoutWindow>,
    pub output: Option<Output>,
    dispatch: tehuti_core::dispatch::State,
    settings_display: crate::settings::Display,
    previous_tick: Instant,
    dim_elapsed: core::time::Duration,
    focus_outline_ids: [smithay::backend::renderer::element::Id; 4],
    popups: smithay::desktop::PopupManager,
    plugins: Option<crate::plugins::Plugins>,
    quit_requested: bool,
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
    layout: tehuti_core::layout::Layout,
    layout_windows: Vec<(tehuti_core::layout::WindowId, LayoutWindow)>,
    next_window: u64,
    windows: tehuti_core::windows::Windows<WlSurface>,
    surfaces: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    _viewporter_state: ViewporterState,
    _fractional_scale_state: FractionalScaleManagerState,
    pub(crate) dmabuf: crate::rendering::DmabufSupport,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    keyboard: KeyboardHandle<Self>,
    controller_session: tehuti_core::controller::State,
    seat: Seat<Self>,
    shortcut_keycodes: Vec<(Keycode, u32)>,
    text_keys: std::collections::HashMap<char, (Keycode, ModifiersState)>,
    pub(crate) text_input: crate::text_input::TextInput,
    keyboard_reserved: i32,
    display: DisplayHandle,
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

fn default_keymap() -> Result<xkb::Keymap, SetupError> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(&context, "", "", "", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
        .ok_or(SetupError::KeymapCompilation)
}

impl Compositor {
    /// Creates the Wayland globals and a seat with one keyboard.
    ///
    /// # Errors
    /// The keyboard keymap fails to load or the desktop entry worker cannot start.
    #[expect(
        clippy::too_many_lines,
        reason = "one flat list of Wayland globals; splitting hides the setup order"
    )]
    pub fn new(display: &DisplayHandle) -> Result<Self, SetupError> {
        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(display, "tehuti");
        let keymap = default_keymap()?;
        let keyboard = seat.add_keyboard(XkbConfig::default(), 200, 25)?;
        let text_keys = text_keys(&keymap);
        let pointer = seat.add_pointer();
        let mut compositor = Self {
            start_time: Instant::now(),
            pointer,
            pointer_location: (100.0, 100.0).into(),
            mouse_mode: false,
            space: Space::default(),
            output: None,
            // Agent launchers title windows Claude/Codex[: project]; mirror them until compositor config exists.
            dispatch: tehuti_core::dispatch::State::new(
                tehuti_core::State::new().with_bar_agents(vec!["Claude".into(), "Codex".into()]),
            ),
            settings_display: crate::settings::Display::default(),
            previous_tick: Instant::now(),
            dim_elapsed: core::time::Duration::ZERO,
            focus_outline_ids: core::array::from_fn(|_| smithay::backend::renderer::element::Id::new()),
            popups: smithay::desktop::PopupManager::default(),
            plugins: None,
            quit_requested: false,
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
            layout: tehuti_core::layout::Layout::new(tehuti_core::layout::LayoutConfig::default()),
            layout_windows: Vec::new(),
            next_window: 0,
            windows: tehuti_core::windows::Windows::new(),
            surfaces: CompositorState::new_v6::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            shm_state: ShmState::new::<Self>(display, vec![]),
            _viewporter_state: ViewporterState::new::<Self>(display),
            _fractional_scale_state: FractionalScaleManagerState::new::<Self>(display),
            dmabuf: crate::rendering::DmabufSupport::default(),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(display),
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(display),
            keyboard,
            controller_session: tehuti_core::controller::State::default(),
            seat,
            shortcut_keycodes: Vec::new(),
            text_keys,
            text_input: crate::text_input::TextInput::new(display),
            keyboard_reserved: 0,
            display: display.clone(),
        };
        compositor
            .keyboard
            .clone()
            .set_keymap_from_string(&mut compositor, keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))?;
        compositor.shortcut_keycodes = (keymap.min_keycode().raw()..=keymap.max_keycode().raw())
            .map(|code| {
                let keycode = Keycode::new(code);
                (keycode, keymap.num_layouts_for_key(keycode))
            })
            .collect();
        Ok(compositor)
    }

    /// Installs contexts before accepting clients.
    pub fn set_contexts(&mut self, contexts: Vec<tehuti_protocol::Context>) {
        *self.dispatch.core_mut() = tehuti_core::State::with_contexts(contexts);
    }

    /// The installed contexts, in priority order.
    #[must_use]
    pub fn contexts(&self) -> &[tehuti_protocol::Context] {
        self.dispatch.core().contexts()
    }

    /// Applies a context reload. A failed reload keeps the current set and names the file.
    pub fn reload_contexts(
        &mut self,
        reload: Result<Vec<tehuti_protocol::Context>, crate::contexts::LoadError>,
    ) {
        match reload {
            Ok(contexts) => {
                tracing::info!(count = contexts.len(), "contexts reloaded");
                self.dispatch.core_mut().replace_contexts(contexts);
                self.update_window_dimming();
                self.show_notice("CONTEXTS RELOADED");
            }
            Err(error) => {
                tracing::warn!(%error, "context reload failed; keeping the current contexts");
                self.show_notice(&error.notice());
            }
        }
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
        self.dispatch.core().pending_decisions()
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
        self.dispatch(DispatchInput::PluginReply { key, result })
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
        if let Some(menu) = self.dispatch.core().menu() {
            return Some(menu.text());
        }
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
            .or_else(|| self.mouse_mode.then(|| "MOUSE MODE".into()))
    }

    pub fn speech_input(&mut self, input: tehuti_core::speech::SpeechInput) {
        let _rejection = self.dispatch(DispatchInput::Speech(input));
    }

    pub(crate) fn plugin_exited(&mut self, plugin: usize) {
        let _rejection = self.dispatch(DispatchInput::PluginExited { plugin });
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
        let elapsed = self.read_clock();
        tracing::trace!(?elapsed, "dispatch tick");
        let _rejection = self.dispatch(DispatchInput::Tick { elapsed });
    }

    pub fn controller_input(&mut self, input: tehuti_core::controller::Input) {
        if matches!(input, tehuti_core::controller::Input::Disconnected) {
            let _rejection = self.dispatch(DispatchInput::ControllerDisconnected);
        }
        let (next, events) =
            tehuti_core::controller::step(core::mem::take(&mut self.controller_session), input);
        self.controller_session = next;
        for event in events {
            self.handle(&event);
        }
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "Smithay pointer coordinates and axis values use floating point"
    )]
    fn pointer_motion(&mut self, x: i32, y: i32) {
        tracing::trace!(x, y, "mouse dispatch motion");
        self.pointer_location.x += f64::from(x) / 1_000_000.0;
        self.pointer_location.y += f64::from(y) / 1_000_000.0;
        self.move_pointer();
    }
    #[expect(
        clippy::float_arithmetic,
        reason = "Smithay pointer coordinates and axis values use floating point"
    )]
    fn pointer_scroll(&mut self, x: i32, y: i32) {
        use smithay::backend::input::{Axis, AxisSource};
        if self.mouse_mode {
            self.move_pointer();
        } else {
            let Some(surface) = self.keyboard.current_focus() else {
                return;
            };
            let Some(window) = self.window_for_root(&surface) else {
                return;
            };
            let geometry = window.geometry();
            if geometry.size.w <= 0 || geometry.size.h <= 0 {
                return;
            }
            let location = (
                f64::from(geometry.loc.x) + f64::from(geometry.size.w) / 2.0,
                f64::from(geometry.loc.y) + f64::from(geometry.size.h) / 2.0,
            )
                .into();
            let event = smithay::input::pointer::MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: self.pointer_time(),
            };
            self.pointer
                .clone()
                .motion(self, Some((surface, (0.0, 0.0).into())), &event);
            self.pointer.clone().frame(self);
        }
        let frame = smithay::input::pointer::AxisFrame::new(self.pointer_time())
            .source(AxisSource::Continuous)
            .value(Axis::Horizontal, f64::from(x) / 1_000_000.0)
            .value(Axis::Vertical, f64::from(y) / 1_000_000.0);
        self.pointer.clone().axis(self, frame);
        self.pointer.clone().frame(self);
        if !self.mouse_mode {
            self.move_pointer();
        }
    }
    fn pointer_click(&mut self, button: tehuti_core::dispatch::PointerButton, pressed: bool) {
        self.move_pointer();
        if pressed && let Some((window, _)) = self.space.element_under(self.pointer_location) {
            let identity = self
                .layout_windows
                .iter()
                .find(|(_, candidate)| candidate == window)
                .map(|(identity, _)| *identity);
            if let Some(identity) = identity {
                if let Err(error) = self.layout.focus_window(identity) {
                    tracing::warn!(%error, "pointer focus failed");
                }
                self.place_layout();
            }
        }
        let event = smithay::input::pointer::ButtonEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: self.pointer_time(),
            button: match button {
                tehuti_core::dispatch::PointerButton::Left => 0x110,
                tehuti_core::dispatch::PointerButton::Right => 0x111,
            },
            state: if pressed {
                smithay::backend::input::ButtonState::Pressed
            } else {
                smithay::backend::input::ButtonState::Released
            },
        };
        self.pointer.clone().button(self, &event);
        self.pointer.clone().frame(self);
    }
    pub fn sample_sticks(&mut self, sticks: tehuti_core::dispatch::Sticks) {
        let _rejection = self.dispatch(DispatchInput::Sticks(sticks));
    }

    #[must_use]
    pub fn cursor_location(&self) -> Option<(f64, f64)> {
        self.mouse_mode
            .then_some((self.pointer_location.x, self.pointer_location.y))
    }

    /// Where to draw the cursor on `output`, in its physical pixels.
    #[must_use]
    pub fn cursor_on(
        &self,
        output: &smithay::output::Output,
    ) -> Option<smithay::utils::Point<f64, smithay::utils::Physical>> {
        let scale = output.current_scale().fractional_scale();
        self.mouse_mode.then(|| self.pointer_location.to_physical(scale))
    }

    fn pointer_time(&self) -> u32 {
        u32::try_from(self.start_time.elapsed().as_millis()).unwrap_or(u32::MAX)
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "Smithay surface hit testing uses floating point coordinates"
    )]
    fn move_pointer(&mut self) {
        if let Some(output) = &self.output
            && let Some(geometry) = self.space.output_geometry(output)
            && geometry.size.w > 0
            && geometry.size.h > 0
        {
            self.pointer_location.x = self.pointer_location.x.clamp(
                f64::from(geometry.loc.x),
                f64::from(geometry.loc.x.saturating_add(geometry.size.w).saturating_sub(1)),
            );
            self.pointer_location.y = self.pointer_location.y.clamp(
                f64::from(geometry.loc.y),
                f64::from(geometry.loc.y.saturating_add(geometry.size.h).saturating_sub(1)),
            );
        }
        let focus = self
            .space
            .element_under(self.pointer_location)
            .and_then(|(window, location)| {
                window
                    .surface_under(
                        (
                            self.pointer_location.x - f64::from(location.x),
                            self.pointer_location.y - f64::from(location.y),
                        ),
                        smithay::desktop::WindowSurfaceType::ALL,
                    )
                    .map(|(surface, offset)| {
                        (
                            surface,
                            (
                                f64::from(location.x.saturating_add(offset.x)),
                                f64::from(location.y.saturating_add(offset.y)),
                            )
                                .into(),
                        )
                    })
            });
        let event = smithay::input::pointer::MotionEvent {
            location: self.pointer_location,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.pointer_time(),
        };
        self.pointer.clone().motion(self, focus, &event);
        self.pointer.clone().frame(self);
    }

    /// Reads the clock once and returns the time since the previous read.
    fn read_clock(&mut self) -> core::time::Duration {
        let now = Instant::now();
        let elapsed = now.duration_since(self.previous_tick);
        self.previous_tick = now;
        elapsed
    }

    /// Runs one event through the core dispatch step and carries out its effects.
    pub fn handle(&mut self, event: &Event) {
        let since_tick = if matches!(event, Event::Focus { .. }) {
            core::time::Duration::ZERO
        } else {
            self.read_clock()
        };
        let input = if let Event::Tick { elapsed } = event {
            DispatchInput::Tick { elapsed: *elapsed }
        } else {
            DispatchInput::Event {
                event: event.clone(),
                elapsed: since_tick,
            }
        };
        let _rejection = self.dispatch(input);
    }

    /// Feeds one input and every reply it asks for until its frame finishes.
    pub(crate) fn dispatch(&mut self, input: DispatchInput) -> Option<tehuti_core::plugin::Rejection> {
        let elapsed = match &input {
            DispatchInput::Tick { elapsed }
            | DispatchInput::Event { elapsed, .. }
            | DispatchInput::ClockRead(elapsed) => *elapsed,
            DispatchInput::DisplayModes { .. }
            | DispatchInput::ModeApplied(_)
            | DispatchInput::Speech(_)
            | DispatchInput::PluginReply { .. }
            | DispatchInput::PluginExited { .. }
            | DispatchInput::TextField { .. }
            | DispatchInput::ControllerDisconnected
            | DispatchInput::Sticks(_)
            | DispatchInput::PluginActive(_)
            | DispatchInput::PluginRequest(_)
            | DispatchInput::Supervised(_)
            | DispatchInput::SecretConfirmation(_)
            | DispatchInput::SpeechSent(_)
            | DispatchInput::SpeechPolled(_)
            | DispatchInput::ForegroundProgram(_)
            | DispatchInput::Resume => core::time::Duration::ZERO,
        };
        self.dim_elapsed = self.dim_elapsed.saturating_add(elapsed);
        let level = self.dispatch.core().level();
        let depth = self.dispatch.busy();
        let mut input = input;
        let mut rejection = None;
        loop {
            let (next, effects) = tehuti_core::dispatch::step(core::mem::take(&mut self.dispatch), input);
            self.dispatch = next;
            let mut reply = DispatchInput::Resume;
            for effect in effects {
                if let DispatchEffect::PluginRejected(rejected) = effect {
                    rejection = Some(rejected);
                } else if let Some(answer) = self.run_effect(effect) {
                    reply = answer;
                }
            }
            if self.dispatch.busy() <= depth {
                if level != self.dispatch.core().level() {
                    self.dim_elapsed = core::time::Duration::ZERO;
                }
                self.update_window_dimming();
                self.reserve_keyboard_height();
                return rejection;
            }
            input = reply;
        }
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "linear colour fade over validated duration"
    )]
    fn update_window_dimming(&self) {
        let (enabled, strength, duration) = self.dispatch.core().dim_config();
        let snapshot = self.layout.snapshot();
        for output in self.space.outputs() {
            let Some(area) = self.space.output_geometry(output) else {
                continue;
            };
            let visible = self
                .space
                .elements()
                .filter(|window| {
                    window.mapped()
                        && self
                            .space
                            .element_bbox(window)
                            .is_some_and(|bounds| bounds.overlaps(area))
                })
                .collect::<Vec<_>>();
            let progress = if duration.is_zero() {
                1.0
            } else {
                (self.dim_elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.0)
            };
            for window in &visible {
                let focused = self.layout_windows.iter().any(|(identity, candidate)| {
                    candidate == *window && snapshot.focused_window == Some(*identity)
                });
                let brightness = if enabled
                    && self.dispatch.core().level() == tehuti_core::Level::App
                    && visible.len() > 1
                    && !focused
                {
                    (1.0 - strength).mul_add(-progress, 1.0)
                } else {
                    1.0
                };
                window.set_brightness(brightness);
            }
        }
    }

    /// Resizes tiled windows when the on-screen keyboard opens or closes.
    fn reserve_keyboard_height(&mut self) {
        let keyboard = self.dispatch.keyboard();
        let height = if keyboard.is_open() {
            crate::osk::logical_height(keyboard)
        } else {
            0
        };
        if height != self.keyboard_reserved {
            self.keyboard_reserved = height;
            self.fill_output();
        }
    }

    /// The on-screen keyboard, for drawing it.
    #[must_use]
    pub const fn on_screen_keyboard(&self) -> &tehuti_core::keyboard::Keyboard {
        self.dispatch.keyboard()
    }

    /// Sends a start or stop to the speech sidecar; `false` when it fails.
    fn send_speech_command(&mut self, command: &tehuti_core::speech::SpeechEffect) -> bool {
        match command {
            tehuti_core::speech::SpeechEffect::Start { session } => {
                tracing::info!(session, "speech session started");
            }
            tehuti_core::speech::SpeechEffect::Stop { session, transcribe } => {
                tracing::info!(session, transcribe, "speech session stopped");
            }
            tehuti_core::speech::SpeechEffect::Speech { .. }
            | tehuti_core::speech::SpeechEffect::Interim { .. }
            | tehuti_core::speech::SpeechEffect::Failed { .. } => {}
        }
        self.speech_process.as_mut().is_none_or(|process| {
            process
                .send(command)
                .inspect_err(|error| tracing::warn!(%error, "speech command failed"))
                .is_ok()
        })
    }

    #[must_use]
    pub fn settings_buffer(
        &self,
        size: (i32, i32),
    ) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> {
        self.settings_display
            .artwork
            .borrow_mut()
            .buffer(self.settings(), size)
    }

    pub fn configure_display_modes(&mut self, modes: Vec<String>, current: usize) {
        let _rejection = self.dispatch(DispatchInput::DisplayModes { modes, current });
    }

    pub fn display_mode_applied(&mut self, current: Option<usize>) {
        self.advance_time();
        let _rejection = self.dispatch(DispatchInput::ModeApplied(current));
    }

    pub fn display_mode_saved(&mut self, result: std::io::Result<()>) {
        if let Err(error) = result {
            tracing::warn!(%error, "display mode persistence failed");
            self.show_notice("mode not saved");
        }
    }

    pub fn take_settings_request(&mut self) -> Option<(usize, bool)> {
        self.settings_display.requests.pop_front()
    }

    #[must_use]
    pub const fn settings(&self) -> &tehuti_core::dispatch::Settings {
        self.dispatch.settings()
    }

    fn run_effect(&mut self, effect: DispatchEffect) -> Option<DispatchInput> {
        match effect {
            DispatchEffect::ReadClock => Some(DispatchInput::ClockRead(self.read_clock())),
            DispatchEffect::InvalidReply => {
                tracing::error!("invalid dispatch reply");
                None
            }
            DispatchEffect::QueryPlugin(event) => Some(DispatchInput::PluginActive(
                self.plugins
                    .as_mut()
                    .is_some_and(|plugins| plugins.active(&event)),
            )),
            DispatchEffect::DispatchPlugin { event, route } => Some(DispatchInput::PluginRequest(
                self.plugins
                    .as_mut()
                    .and_then(|plugins| plugins.dispatch(&event, route)),
            )),
            DispatchEffect::Supervise => Some(DispatchInput::Supervised(
                self.plugins
                    .as_mut()
                    .map_or_else(Vec::new, crate::plugins::Plugins::supervise),
            )),
            DispatchEffect::ConfirmSecret(state) => Some(DispatchInput::SecretConfirmation(
                self.handle_secret_confirmation_button(state),
            )),
            DispatchEffect::SpeechCommand(command) => {
                Some(DispatchInput::SpeechSent(self.send_speech_command(&command)))
            }
            DispatchEffect::PollSpeech(elapsed) => {
                Some(DispatchInput::SpeechPolled(self.poll_speech(elapsed)))
            }
            DispatchEffect::QueryForegroundProgram => Some(DispatchInput::ForegroundProgram(
                self.focused_terminal_runs_program(),
            )),
            effect @ (DispatchEffect::SettingsMode(_)
            | DispatchEffect::PersistMode(_)
            | DispatchEffect::PasswordSpeechIgnored
            | DispatchEffect::RunMenuCommand(_)
            | DispatchEffect::MovePointer { .. }
            | DispatchEffect::Scroll { .. }
            | DispatchEffect::Click { .. }
            | DispatchEffect::MouseMode(_)
            | DispatchEffect::PollSources
            | DispatchEffect::Chord(_)
            | DispatchEffect::SouthPressed(_)
            | DispatchEffect::ResetSecretFocus
            | DispatchEffect::Apply(_)
            | DispatchEffect::PluginAction(_)
            | DispatchEffect::PluginRejected(_)
            | DispatchEffect::Handle(_)
            | DispatchEffect::SpeechFailed { .. }
            | DispatchEffect::TranscriptReceived { .. }
            | DispatchEffect::SpeechRouted { .. }) => {
                self.carry_out(effect);
                None
            }
        }
    }

    fn poll_speech(&mut self, elapsed: core::time::Duration) -> Vec<tehuti_core::speech::SpeechInput> {
        let inputs = self
            .speech_process
            .as_mut()
            .map_or_else(Vec::new, |process| process.poll(elapsed));
        while let Some(notice) = self
            .speech_process
            .as_mut()
            .and_then(crate::speech::SpeechProcess::take_notice)
        {
            self.show_notice(notice);
        }
        inputs
    }

    fn queue_settings(&mut self, index: usize, persist: bool) {
        self.settings_display.requests.push_back((index, persist));
    }

    fn run_menu_command(&mut self, command: tehuti_core::menu::Command) {
        if let Err(error) = crate::system_menu::execute(command) {
            tracing::warn!(?command, %error, "system menu command failed");
            self.show_notice("MENU COMMAND UNAVAILABLE");
        }
    }

    /// Runs an effect that needs no reply; `run_effect` answers the queries.
    fn carry_out(&mut self, effect: DispatchEffect) {
        use tehuti_core::speech::SpeechFailure;
        match effect {
            DispatchEffect::SettingsMode(index) => self.queue_settings(index, false),
            DispatchEffect::PersistMode(index) => self.queue_settings(index, true),
            DispatchEffect::PasswordSpeechIgnored => {
                tracing::info!("push-to-talk ignored: password field active");
            }
            DispatchEffect::RunMenuCommand(command) => self.run_menu_command(command),
            DispatchEffect::MovePointer { x, y } => self.pointer_motion(x, y),
            DispatchEffect::Scroll { x, y } => self.pointer_scroll(x, y),
            DispatchEffect::Click { button, pressed } => self.pointer_click(button, pressed),
            DispatchEffect::MouseMode(active) => {
                self.mouse_mode = active;
                self.show_notice(if active { "MOUSE MODE" } else { "MOUSE MODE OFF" });
                self.move_pointer();
            }
            DispatchEffect::PollSources => {
                self.poll_secret_lookup();
                self.poll_launchers();
                self.desktop_entries.poll();
            }
            DispatchEffect::SouthPressed(pressed) => self.secret_confirmation.south_pressed = pressed,
            DispatchEffect::ResetSecretFocus => {
                self.reset_secret_confirmation_for_focus();
            }
            DispatchEffect::Apply(action) => self.apply(&action),
            DispatchEffect::PluginAction(action) => self.apply_plugin_action(&action),
            DispatchEffect::Chord(chord) => self.tap_chord(chord),
            DispatchEffect::ReadClock
            | DispatchEffect::InvalidReply
            | DispatchEffect::PluginRejected(_)
            | DispatchEffect::QueryPlugin(_)
            | DispatchEffect::DispatchPlugin { .. }
            | DispatchEffect::Supervise
            | DispatchEffect::QueryForegroundProgram
            | DispatchEffect::ConfirmSecret(_)
            | DispatchEffect::SpeechCommand(_)
            | DispatchEffect::PollSpeech(_) => {}
            DispatchEffect::Handle(event) => {
                self.handle(&event);
            }
            DispatchEffect::SpeechFailed { session, reason } => {
                tracing::warn!(session, ?reason, "speech transcription failed");
                self.show_notice(match reason {
                    SpeechFailure::Silent => "MIC SILENT",
                    SpeechFailure::Transcription => "SPEECH FAILED",
                });
            }
            DispatchEffect::TranscriptReceived {
                session,
                text,
                admitted,
            } => {
                log_transcript(session, &text, admitted);
            }
            DispatchEffect::SpeechRouted { text, first, plugin } => {
                log_speech_route(&text, first.as_ref(), plugin);
            }
        }
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
        if self.dispatch.core().menu().is_some() {
            return None;
        }
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
            Action::Key { key, mods } => {
                self.tap_with_modifiers(*key, mods);
                let (app_id, title) = self.focused_window_label();
                tracing::info!(?key, ?app_id, ?title, "key delivered to focused window");
            }
            Action::Text { text } => {
                let method = if self.text_input.send(text.as_str()) {
                    "text-input"
                } else {
                    self.type_text(text.as_str());
                    "keys"
                };
                let (app_id, title) = self.focused_window_label();
                tracing::info!(
                    characters = text.as_str().chars().count(),
                    method,
                    ?app_id,
                    ?title,
                    "text delivered to focused window"
                );
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
                if let Some(identity) = self.layout.snapshot().focused_window {
                    self.remove_layout_window(identity, true);
                }
            }
            Action::Layout { action } => self.apply_layout(*action),
            Action::Notice { text } => {
                self.show_notice(text.as_str());
            }
        }
    }

    /// App id and title of the window holding keyboard focus, for delivery logs.
    fn focused_window_label(&self) -> (Option<String>, Option<String>) {
        self.keyboard
            .current_focus()
            .filter(smithay::reexports::wayland_server::Resource::is_alive)
            .and_then(|focus| {
                smithay::wayland::compositor::with_states(&focus, |states| {
                    states
                        .data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .map(|attributes| {
                            let attributes = attributes
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            (attributes.app_id.clone(), attributes.title.clone())
                        })
                })
            })
            .unwrap_or((None, None))
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

    /// Taps a key with Control or Alt held, from the on-screen keyboard's terminal strip.
    fn tap_chord(&mut self, chord: tehuti_core::keyboard::Chord) {
        use tehuti_core::keyboard::ChordKey;
        use tehuti_protocol::Modifier;
        let mut mods = Vec::new();
        if chord.modifiers.control {
            mods.push(Modifier::Ctrl);
        }
        if chord.modifiers.alt {
            mods.push(Modifier::Alt);
        }
        let keycode = match chord.key {
            ChordKey::Named(key) => Keycode::new(key.evdev_code().saturating_add(XKB_KEYCODE_OFFSET)),
            ChordKey::Char(character) => {
                let Some((keycode, level)) = self.text_keys.get(&character).copied() else {
                    tracing::warn!(?character, "chord character unavailable in keymap; skipped");
                    return;
                };
                if level.shift {
                    mods.push(Modifier::Shift);
                }
                keycode
            }
        };
        self.tap_keycode_with_modifiers(keycode, &mods);
        tracing::info!(?chord, "chord delivered to focused window");
    }

    /// Whether the focused client's shell has a program in the foreground; false when unknown.
    fn focused_terminal_runs_program(&self) -> bool {
        let pid = self
            .keyboard
            .current_focus()
            .and_then(|surface| smithay::reexports::wayland_server::Resource::client(&surface))
            .and_then(|client| client.get_credentials(&self.display).ok())
            .map(|credentials| credentials.pid);
        pid.is_some_and(crate::foreground::runs_program)
    }

    /// Resolves a keysym name to a keycode in the active layout.
    fn resolve_keysym(&mut self, name: &str) -> Option<Keycode> {
        let keyboard = self.keyboard.clone();
        let codes = self.shortcut_keycodes.clone();
        keyboard.with_xkb_state(self, |context| {
            let xkb = context
                .xkb()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let symbol = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
            codes.iter().find_map(|(code, layouts)| {
                let layout = smithay::input::keyboard::Layout(xkb.active_layout().0.checked_rem(*layouts)?);
                xkb.raw_syms_for_key_in_layout(*code, layout)
                    .contains(&symbol)
                    .then_some(*code)
            })
        })
    }

    fn tap_with_modifiers(&mut self, key: KeyName, mods: &[tehuti_protocol::Modifier]) {
        let Some(keycode) = self.resolve_keysym(key.keysym_name()) else {
            tracing::warn!(?key, "shortcut unavailable in active keymap");
            return;
        };
        self.tap_keycode_with_modifiers(keycode, mods);
    }

    /// The one way to send a modifier: press the real modifier keys so xkb serializes the mask.
    fn tap_keycode_with_modifiers(&mut self, keycode: Keycode, mods: &[tehuti_protocol::Modifier]) {
        use tehuti_protocol::Modifier;
        let held = [
            (Modifier::Ctrl, "Control_L"),
            (Modifier::Alt, "Alt_L"),
            (Modifier::Shift, "Shift_L"),
            (Modifier::Super, "Super_L"),
        ]
        .into_iter()
        .filter(|(modifier, _)| mods.contains(modifier))
        .map(|(_, name)| self.resolve_keysym(name))
        .collect::<Option<Vec<_>>>();
        let Some(held) = held else {
            tracing::warn!(?mods, "modifier unavailable in active keymap");
            return;
        };
        for code in &held {
            self.send_keycode(*code, smithay::backend::input::KeyState::Pressed);
        }
        self.tap_keycode(keycode);
        for code in held.iter().rev() {
            self.send_keycode(*code, smithay::backend::input::KeyState::Released);
        }
    }

    fn tap_keycode(&mut self, keycode: Keycode) {
        for state in [
            smithay::backend::input::KeyState::Pressed,
            smithay::backend::input::KeyState::Released,
        ] {
            self.send_keycode(keycode, state);
        }
    }

    /// Forwards one key from the host keyboard to the focused client.
    pub fn host_key(&mut self, keycode: Keycode, pressed: bool) {
        let state = if pressed {
            smithay::backend::input::KeyState::Pressed
        } else {
            smithay::backend::input::KeyState::Released
        };
        self.send_keycode(keycode, state);
    }

    /// Releases every held host key, so a key held when the host window loses focus does not stick.
    pub fn release_host_keys(&mut self) {
        for keycode in self.keyboard.pressed_keys() {
            self.send_keycode(keycode, smithay::backend::input::KeyState::Released);
        }
    }

    fn send_keycode(&mut self, keycode: Keycode, state: smithay::backend::input::KeyState) {
        let keyboard = self.keyboard.clone();
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

    fn apply_layout(&mut self, action: tehuti_protocol::LayoutAction) {
        self.layout.apply(tehuti_core::layout::command_for(action));
        self.place_layout();
    }

    /// The tiling layout every mapped window is placed by.
    #[must_use]
    pub fn layout_snapshot(&self) -> tehuti_core::layout::Snapshot {
        self.layout.snapshot()
    }

    #[must_use]
    pub fn focus_outline(
        &self,
        output: &Output,
    ) -> Vec<smithay::backend::renderer::element::solid::SolidColorRenderElement> {
        use smithay::utils::Rectangle;
        let snapshot = self.layout.snapshot();
        let fullscreen = snapshot
            .outputs
            .iter()
            .flat_map(|snapshot_output| &snapshot_output.workspaces)
            .flat_map(|workspace| &workspace.columns)
            .flat_map(|column| &column.tiles)
            .any(|tile| {
                Some(tile.window) == snapshot.focused_window
                    && tile.mode == tehuti_core::layout::TileMode::Fullscreen
            });
        if fullscreen {
            return Vec::new();
        }
        let Some((_, window)) = self
            .layout_windows
            .iter()
            .find(|(identity, _)| Some(*identity) == snapshot.focused_window)
        else {
            return Vec::new();
        };
        let (Some(location), Some(area)) = (
            self.space.element_location(window),
            self.space.output_geometry(output),
        ) else {
            return Vec::new();
        };
        if !window.mapped() {
            return Vec::new();
        }
        let location = (
            location.x.saturating_sub(area.loc.x),
            location.y.saturating_sub(area.loc.y),
        );
        let bounds = Rectangle::new(location.into(), window.layout_size());
        crate::focus_outline::elements(
            bounds,
            output.current_scale().fractional_scale(),
            &self.focus_outline_ids,
        )
    }

    #[must_use]
    pub fn voice_bar(&self) -> tehuti_core::bar::Bar {
        let snapshot = self.layout.snapshot();
        let windows = snapshot
            .outputs
            .iter()
            .filter_map(|output| output.workspaces.get(output.active_workspace))
            .flat_map(|workspace| &workspace.columns)
            .flat_map(|column| &column.tiles)
            .filter_map(|tile| {
                let (_, window) = self
                    .layout_windows
                    .iter()
                    .find(|(identity, _)| *identity == tile.window)?;
                let toplevel = window.toplevel()?;
                let title = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
                    states
                        .data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .map(|attributes| {
                            let attributes = attributes
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            attributes
                                .title
                                .clone()
                                .filter(|title| !title.is_empty())
                                .or_else(|| attributes.app_id.clone())
                                .unwrap_or_else(|| "Untitled".into())
                        })
                })?;
                Some((title, snapshot.focused_window == Some(tile.window)))
            })
            .collect();
        self.dispatch.core().bar().clone().with_windows(windows)
    }

    /// Updates the layout to the output's logical size and configures its windows.
    #[expect(
        clippy::float_arithmetic,
        reason = "reserve the bar in logical output coordinates"
    )]
    pub fn fill_output(&mut self) {
        use tehuti_core::layout::{OutputGeometry, OutputId, Rect};
        let size = self.output.as_ref().and_then(|output| {
            output.current_mode().map(|mode| {
                mode.size
                    .to_f64()
                    .to_logical(output.current_scale().fractional_scale())
            })
        });
        let area = size.map_or(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1280.0,
                height: 720.0,
            },
            |size| Rect {
                x: 0.0,
                y: 0.0,
                width: size.w,
                height: size.h,
            },
        );
        let geometry = OutputGeometry {
            area,
            usable: Rect {
                height: (area.height
                    - f64::from(tehuti_core::bar::HEIGHT)
                    - f64::from(self.keyboard_reserved))
                .max(1.0),
                ..area
            },
        };
        let result = if self.layout.snapshot().outputs.is_empty() {
            self.layout.add_output(OutputId(0), geometry, None)
        } else {
            self.layout.update_output(OutputId(0), geometry)
        };
        if let Err(error) = result {
            tracing::warn!(%error, "output layout failed");
        }
        self.place_layout();
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "client positions include the layout border"
    )]
    fn place_layout(&mut self) {
        let snapshot = self.layout.snapshot();
        for (identity, window) in &self.layout_windows {
            let tile = snapshot.outputs.iter().find_map(|output| {
                output
                    .workspaces
                    .get(output.active_workspace)
                    .and_then(|workspace| {
                        workspace.columns.iter().find_map(|column| {
                            column.tiles.iter().enumerate().find_map(|(index, tile)| {
                                (tile.window == *identity
                                    && (column.display == tehuti_core::layout::ColumnDisplay::Normal
                                        || index == column.focused_tile))
                                    .then_some((tile, output.border))
                            })
                        })
                    })
            });
            let Some((tile, border)) = tile else {
                self.space.unmap_elem(window);
                continue;
            };
            let border = if matches!(
                tile.mode,
                tehuti_core::layout::TileMode::Fullscreen | tehuti_core::layout::TileMode::MaximizedToEdges
            ) {
                0.0
            } else {
                border
            };
            window.set_size(
                smithay::utils::Size::<f64, smithay::utils::Logical>::from((
                    tile.requested_client.width,
                    tile.requested_client.height,
                ))
                .to_i32_round(),
            );
            self.space.map_element(
                window.clone(),
                smithay::utils::Point::<f64, smithay::utils::Logical>::from((
                    tile.tile.x + border,
                    tile.tile.y + border,
                ))
                .to_i32_round(),
                snapshot.focused_window == Some(*identity),
            );
            // Space only reports output entry once a buffer gives the window a
            // view. Clients such as mpv size their first buffer from the output
            // scale they learn on entry, so announce it before the first
            // configure instead of after the first (wrongly scaled) frame.
            if let (Some(output), Some(toplevel)) = (&self.output, window.toplevel()) {
                output.enter(toplevel.wl_surface());
                Self::announce_scale(output, toplevel.wl_surface());
            }
            if let Some(toplevel) = window.toplevel() {
                Self::configure_layout_window(
                    toplevel,
                    tile,
                    snapshot.focused_window == Some(*identity),
                    !window.size_held(),
                );
            }
        }
        self.sync_window_focus();
        self.update_window_dimming();
    }

    fn configure_layout_window(
        toplevel: &ToplevelSurface,
        tile: &tehuti_core::layout::TileView,
        focused: bool,
        send: bool,
    ) {
        toplevel.with_pending_state(|pending| {
            pending.size = Some(
                smithay::utils::Size::<f64, smithay::utils::Logical>::from((
                    tile.requested_client.width,
                    tile.requested_client.height,
                ))
                .to_i32_round(),
            );
            for (state, enabled) in [
                (xdg_toplevel::State::Fullscreen, tile.fullscreen_requested),
                (xdg_toplevel::State::Activated, focused),
            ] {
                if enabled {
                    let _changed = pending.states.set(state);
                } else {
                    let _changed = pending.states.unset(state);
                }
            }
        });
        if send && toplevel.is_initial_configure_sent() {
            let _serial = toplevel.send_pending_configure();
        }
    }

    fn remove_layout_window(&mut self, identity: tehuti_core::layout::WindowId, close: bool) {
        if let Some(index) = self
            .layout_windows
            .iter()
            .position(|(window_id, _)| *window_id == identity)
        {
            let (_, window) = self.layout_windows.remove(index);
            if let Some(toplevel) = window.toplevel() {
                if close {
                    toplevel.send_close();
                }
                let _effect = self
                    .windows
                    .step(tehuti_core::windows::Input::Unmap(toplevel.wl_surface().clone()));
            }
            self.space.unmap_elem(&window);
            if let Err(error) = self.layout.remove_window(identity) {
                tracing::warn!(%error, "window removal failed");
            }
            self.place_layout();
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
        let identity = self.layout.snapshot().focused_window;
        let focus = self
            .layout_windows
            .iter()
            .find(|(window_id, _)| Some(*window_id) == identity)
            .and_then(|(_, window)| window.toplevel().map(|toplevel| toplevel.wl_surface().clone()));
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

    fn window_for_root(&self, root: &WlSurface) -> Option<&LayoutWindow> {
        self.layout_windows
            .iter()
            .map(|(_, window)| window)
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == root)
            })
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        for child in &mut self.launchers {
            crate::shutdown::terminate(child, false);
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

    fn new_surface(&mut self, surface: &WlSurface) {
        if let Some(output) = &self.output {
            Self::announce_scale(output, surface);
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);
        if let Some(smithay::desktop::PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
            && let Err(error) = popup.send_configure()
        {
            tracing::warn!(%error, "popup configure failed");
        }
        self.popups.cleanup();
        if is_sync_subsurface(surface) {
            return;
        }
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if let Some(window) = self.window_for_root(&root) {
            window.on_commit();
            if let Some(toplevel) = window.toplevel() {
                if !toplevel.is_initial_configure_sent() {
                    window.hold_size(true);
                    toplevel.with_pending_state(|pending| pending.size = None);
                    let _serial = toplevel.send_configure();
                    toplevel.with_pending_state(|pending| pending.size = Some(window.layout_size()));
                } else if window.size_held() && window.mapped() {
                    window.hold_size(false);
                    let _serial = toplevel.send_pending_configure();
                }
            }
        }
    }
}

impl FractionalScaleHandler for Compositor {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        if let Some(output) = &self.output {
            Self::announce_scale(output, &surface);
        }
    }
}

impl Compositor {
    /// Tell a surface the output's scale, both as an integer
    /// (`wl_surface.preferred_buffer_scale`) and as `wp_fractional_scale_v1`.
    fn announce_scale(output: &Output, surface: &WlSurface) {
        let scale = output.current_scale();
        smithay::wayland::compositor::with_states(surface, |states| {
            smithay::wayland::compositor::send_surface_state(
                surface,
                states,
                scale.integer_scale(),
                output.current_transform(),
            );
            with_fractional_scale(states, |fractional| {
                fractional.set_preferred_scale(scale.fractional_scale());
            });
        });
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
        self.fill_output();
        let identity = tehuti_core::layout::WindowId(self.next_window);
        self.next_window = self.next_window.saturating_add(1);
        match self
            .layout
            .add_window(identity, tehuti_core::layout::WindowRules::default())
        {
            Ok(()) => {
                self.layout_windows
                    .push((identity, LayoutWindow::new(Window::new_wayland_window(surface))));
                self.place_layout();
            }
            Err(error) => tracing::warn!(%error, "window layout failed"),
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.update_focus(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.update_focus(&surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let identity = self.layout_windows.iter().find_map(|(identity, window)| {
            window
                .toplevel()
                .filter(|toplevel| toplevel.wl_surface() == surface.wl_surface())
                .map(|_| *identity)
        });
        if let Some(identity) = identity {
            self.remove_layout_window(identity, false);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|pending| pending.geometry = positioner.get_geometry());
        if let Err(error) = self.popups.track_popup(smithay::desktop::PopupKind::Xdg(surface)) {
            tracing::warn!(%error, "popup tracking failed");
        }
    }

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
        let _rejection = self.dispatch(DispatchInput::TextField {
            active: false,
            changed: false,
            kind: tehuti_core::keyboard::FieldKind::Normal,
        });
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
delegate_viewporter!(Compositor);
delegate_fractional_scale!(Compositor);
delegate_xdg_shell!(Compositor);
delegate_seat!(Compositor);
delegate_data_device!(Compositor);
delegate_output!(Compositor);

fn log_speech_route(text: &tehuti_protocol::Text, first: Option<&Action>, plugin: bool) {
    match first {
        Some(Action::Notice { text: notice }) if notice.as_str() == "NO SPEECH MAPPING" => {
            tracing::info!(text = text.as_str(), "speech has no mapping");
        }
        Some(action) => tracing::info!(text = text.as_str(), ?action, "speech mapped"),
        None if plugin => tracing::info!(text = text.as_str(), "speech routed to a plugin"),
        None => tracing::info!(text = text.as_str(), "speech produced no action"),
    }
}

fn log_transcript(session: u64, text: &tehuti_protocol::Text, admitted: bool) {
    tracing::info!(session, text = text.as_str(), "speech transcript received");
    if !admitted {
        tracing::info!("speech transcript dropped for a stale session");
    }
}

#[cfg(test)]
mod tests {
    use super::Compositor;
    use core::time::Duration;
    use smithay::reexports::wayland_server::Display;
    use tehuti_protocol::Event;

    #[test]
    fn polling_speech_shows_the_process_outage_notice() {
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        compositor.set_speech(crate::speech::SpeechProcess::new(vec![
            directory.path().join("missing-speech").into_os_string(),
        ]));
        assert_eq!(compositor.visible_notice_text(), None);

        compositor.handle(&Event::Tick {
            elapsed: Duration::ZERO,
        });

        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("Speech is down, restarting")
        );
        assert!(compositor.speech_notice_visible());
    }
}
