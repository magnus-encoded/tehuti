//! Smithay state and protocol handlers for one output and its tiling layout.

use std::time::Instant;

use crate::layout_window::LayoutWindow;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_fractional_scale, delegate_output, delegate_seat,
    delegate_shm, delegate_viewporter, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{Space, Window},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, KeyboardHandle, KeyboardTarget, Keycode, ModifiersState, XkbConfig, xkb},
        pointer::CursorImageStatus,
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
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
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            decoration::{XdgDecorationHandler, XdgDecorationState},
        },
        shm::{ShmHandler, ShmState},
        viewporter::ViewporterState,
    },
};
use tehuti_core::dispatch::{Effect as DispatchEffect, Input as DispatchInput};
use tehuti_protocol::{Action, Event, KeyName};

const fn dispatch_elapsed(input: &DispatchInput) -> core::time::Duration {
    match input {
        DispatchInput::Tick { elapsed }
        | DispatchInput::Event { elapsed, .. }
        | DispatchInput::ClockRead(elapsed) => *elapsed,
        DispatchInput::Menu(_)
        | DispatchInput::Launcher(_)
        | DispatchInput::CloseInvitation
        | DispatchInput::ChoiceList(_)
        | DispatchInput::FocusTitle { .. }
        | DispatchInput::DisplayModes { .. }
        | DispatchInput::BarFont(_)
        | DispatchInput::DisplayScale(_)
        | DispatchInput::ModeApplied(_)
        | DispatchInput::Controllers(_)
        | DispatchInput::Microphones(_)
        | DispatchInput::InputChoices { .. }
        | DispatchInput::Speech(_)
        | DispatchInput::PluginReply { .. }
        | DispatchInput::PluginExited { .. }
        | DispatchInput::TextField { .. }
        | DispatchInput::ControllerDisconnected
        | DispatchInput::SecretPrompt(_)
        | DispatchInput::Sticks(_)
        | DispatchInput::PluginActive(_)
        | DispatchInput::PluginRequest(_)
        | DispatchInput::Supervised(_)
        | DispatchInput::SecretConfirmation(_)
        | DispatchInput::SpeechSent(_)
        | DispatchInput::SpeechPolled(_)
        | DispatchInput::Wallpapers { .. }
        | DispatchInput::WallpaperSaved(_)
        | DispatchInput::Resume => core::time::Duration::ZERO,
    }
}

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
    confirm_pressed: bool,
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
    menu_config: crate::system_menu::Config,
    settings_display: crate::settings::Display,
    bar_font_store: Option<crate::display_mode_store::Store>,
    /// The pad that drives Tehuti; previews before confirmation too.
    controller_choice: tehuti_core::dispatch::ControllerChoice,
    controller_names: Vec<String>,
    input_choices: Option<crate::input_choice_store::Store>,
    microphone_listing: Option<std::sync::mpsc::Receiver<Vec<tehuti_core::dispatch::Microphone>>>,
    previous_tick: Instant,
    driver_clock: crate::driver::Clock,
    driver_elapsed: core::time::Duration,
    dim_elapsed: core::time::Duration,
    focus_outline_ids: [smithay::backend::renderer::element::Id; 4],
    popups: smithay::desktop::PopupManager,
    plugins: Option<crate::plugins::Plugins>,
    quit_requested: bool,
    session_exit_status: Option<u8>,
    restart_requested: bool,
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
    destination_paths: crate::destinations::Paths,
    destinations: crate::destinations::Table,
    launcher_routes: Vec<crate::destinations::Route>,
    invitation: Invitation,
    destination_windows: Vec<crate::destinations::Window>,
    speech_context: Option<String>,
    launch_socket: Option<std::ffi::OsString>,
    layout: tehuti_core::layout::Layout,
    layout_windows: Vec<(tehuti_core::layout::WindowId, LayoutWindow)>,
    next_window: u64,
    windows: tehuti_core::windows::Windows<WlSurface>,
    surfaces: CompositorState,
    xdg_shell_state: XdgShellState,
    decoration_state: Option<XdgDecorationState>,
    shm_state: ShmState,
    _viewporter_state: ViewporterState,
    _fractional_scale_state: FractionalScaleManagerState,
    pub(crate) dmabuf: crate::rendering::DmabufSupport,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    keyboard: KeyboardHandle<Self>,
    window_focus: Option<(WlSurface, Option<String>)>,
    controller_session: tehuti_core::controller::State,
    seat: Seat<Self>,
    shortcut_keycodes: Vec<(Keycode, u32)>,
    text_keys: std::collections::HashMap<char, (Keycode, ModifiersState)>,
    pub(crate) text_input: crate::text_input::TextInput,
    keyboard_reserved: i32,
    /// Keys a held pad button keeps down, with the keycode they went down on.
    held_pad_keys: Vec<(KeyName, Keycode)>,
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
        // Clients repeat held keys themselves; a slow client can miss the release and repeat for a long time.
        // TEHUTI_KEY_REPEAT_RATE=0 turns repeat off.
        let repeat = |name: &str, default: i32| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|value| *value >= 0)
                .unwrap_or(default)
        };
        let keyboard = seat.add_keyboard(
            XkbConfig::default(),
            repeat("TEHUTI_KEY_REPEAT_DELAY", 200),
            repeat("TEHUTI_KEY_REPEAT_RATE", 25),
        )?;
        let text_keys = text_keys(&keymap);
        let pointer = seat.add_pointer();
        let mut compositor = Self {
            menu_config: crate::system_menu::Config::default(),
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
            bar_font_store: None,
            controller_choice: tehuti_core::dispatch::ControllerChoice::Auto,
            controller_names: Vec::new(),
            input_choices: None,
            microphone_listing: None,
            driver_clock: crate::driver::Clock::Live,
            driver_elapsed: core::time::Duration::ZERO,
            previous_tick: Instant::now(),
            dim_elapsed: core::time::Duration::ZERO,
            focus_outline_ids: core::array::from_fn(|_| smithay::backend::renderer::element::Id::new()),
            popups: smithay::desktop::PopupManager::default(),
            plugins: None,
            quit_requested: false,
            session_exit_status: None,
            restart_requested: false,
            speech_process: None,
            secret_gate: tehuti_core::secret::SecretConfirmationGate::new(),
            secret_provider: Some(RedactedSecretProvider(Box::new(
                tehuti_core::secret::DenySecretProvider,
            ))),
            secret_lookup: None,
            window_focus: None,
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
            destination_paths: crate::destinations::Paths::default(),
            destinations: crate::destinations::Table::default(),
            launcher_routes: Vec::new(),
            invitation: Invitation::Off,
            destination_windows: Vec::new(),
            speech_context: None,
            layout_windows: Vec::new(),
            next_window: 0,
            windows: tehuti_core::windows::Windows::new(),
            surfaces: CompositorState::new_v6::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            decoration_state: None,
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
            held_pad_keys: Vec::new(),
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
        let (names, locked, current) = crate::wallpaper::choices();
        let _rejection = compositor.dispatch(DispatchInput::Wallpapers {
            names,
            locked,
            current,
        });
        match crate::system_menu::load() {
            Ok(config) => compositor.menu_config = config,
            Err(error) => compositor.show_notice(&format!("MENU CONFIG: {error}")),
        }
        let notices = compositor.menu_config.notices().join("\n");
        if !notices.is_empty() {
            tracing::warn!(%notices, "invalid menu entries skipped");
            compositor.show_notice(&notices);
        }
        compositor
            .dispatch
            .core_mut()
            .set_menu_entries(compositor.menu_config.labels());
        compositor.rebuild_destinations();
        Ok(compositor)
    }

    const fn advance_clocks(&mut self, elapsed: core::time::Duration) {
        self.dim_elapsed = self.dim_elapsed.saturating_add(elapsed);
        if !matches!(self.driver_clock, crate::driver::Clock::Live) {
            self.driver_elapsed = self.driver_elapsed.saturating_add(elapsed);
        }
    }

    fn clock_now(&self) -> Instant {
        if matches!(self.driver_clock, crate::driver::Clock::Live) {
            Instant::now()
        } else {
            self.start_time
                .checked_add(self.driver_elapsed)
                .unwrap_or(self.start_time)
        }
    }

    fn clock_elapsed(&self) -> core::time::Duration {
        self.clock_now().saturating_duration_since(self.start_time)
    }

    pub(crate) const fn set_driver_input(&mut self, active: bool) {
        self.driver_clock = if active {
            crate::driver::Clock::Synthetic
        } else {
            crate::driver::Clock::Manual
        };
    }

    pub(crate) const fn use_driver_clock(&mut self) {
        self.driver_clock = crate::driver::Clock::Manual;
    }

    pub(crate) fn driver_action(&mut self, action: &Action) {
        self.apply(action);
    }

    pub(crate) fn driver_scene(&self) -> serde_json::Value {
        use serde_json::json;
        let focused = self.layout.snapshot().focused_window;
        let windows: Vec<_> = self.layout_windows.iter().map(|(identity, window)| {
            let (app_id, title) = window.toplevel().and_then(|surface| self.focus_event(surface)).and_then(|event| {
                if let Event::Focus { app_id, title } = event { Some((app_id, title)) } else { None }
            }).unwrap_or_default();
            let location = self.space.element_location(window).unwrap_or_default();
            let size = window.layout_size();
            json!({"id": identity.0, "app_id": app_id, "title": title, "mapped": window.mapped(),
                "focused": focused == Some(*identity), "geometry": {"x": location.x, "y": location.y, "width": size.w, "height": size.h}})
        }).collect();
        let launcher = self.choice_list().filter(|list| list.is_open()).map(|list| {
            let rows: Vec<_> = list
                .candidates()
                .iter()
                .map(|row| json!({"name": row.candidate.name, "meta": row.candidate.meta}))
                .collect();
            let selected = match list.selected() {
                Some(tehuti_core::choice_list::Choice::Candidate(index)) => json!(index),
                Some(tehuti_core::choice_list::Choice::Create) => json!("create"),
                None => serde_json::Value::Null,
            };
            json!({"rows": rows, "selection": selected, "layout": self.output.as_ref().and_then(smithay::output::Output::current_mode).and_then(|mode| crate::choice_list::scene(list, self.pad_family(), (mode.size.w, mode.size.h)))})
        });
        let bar: Vec<_> = self
            .voice_bar()
            .segments()
            .iter()
            .map(|segment| segment.text.clone())
            .collect();
        json!({"version": 1, "clock_ms": self.driver_elapsed.as_millis(), "windows": windows, "focus": focused.map(|identity| identity.0),
            "level": format!("{:?}", self.dispatch.core().level()), "launcher": launcher,
            "overlays": {"keyboard": self.on_screen_keyboard().is_open(), "settings": self.settings().open, "menu": self.dispatch.core().menu().is_some()},
            "keyboard": self.on_screen_keyboard().is_open(), "notice": self.visible_notice_text(), "bar": bar})
    }

    /// Installs contexts before accepting clients.
    pub fn set_contexts(&mut self, mut contexts: Vec<tehuti_protocol::Context>) {
        crate::speech_vocabulary::configure_projects(&mut contexts, &self.destination_paths.projects);
        *self.dispatch.core_mut() = tehuti_core::State::with_contexts(contexts);
        self.dispatch
            .core_mut()
            .set_menu_entries(self.menu_config.labels());
        self.window_focus = None;
        self.sync_window_focus();
        self.rebuild_destinations();
        crate::wallpaper::set_focus(self.dispatch.desktop_focus());
    }

    /// Names ticker buttons after the pad that sent the latest input.
    pub fn set_pad_family(&mut self, family: tehuti_core::pad::PadFamily) {
        self.dispatch.core_mut().set_pad_family(family);
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
            Ok(mut contexts) => {
                crate::speech_vocabulary::configure_projects(&mut contexts, &self.destination_paths.projects);
                tracing::info!(count = contexts.len(), "contexts reloaded");
                self.dispatch.core_mut().replace_contexts(contexts);
                self.sync_destination_windows();
                crate::wallpaper::set_focus(self.dispatch.desktop_focus());
                self.update_window_dimming();
                self.show_notice("CONTEXTS RELOADED");
                self.rebuild_destinations();
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

    pub fn set_speech(&mut self, mut process: crate::speech::SpeechProcess) {
        if let Err(error) = process.set_vocabulary(&self.destination_vocabulary()) {
            tracing::warn!(%error, "speech vocabulary unavailable");
        }
        self.speech_process = Some(process);
    }

    /// Configures the shell's discovery directories and rebuilds its table.
    pub fn configure_destinations(&mut self, paths: crate::destinations::Paths) {
        self.destination_paths = paths;
        self.rebuild_destinations();
    }

    #[must_use]
    pub const fn destinations(&self) -> &crate::destinations::Table {
        &self.destinations
    }

    fn rebuild_destinations(&mut self) {
        let paths = &self.destination_paths;
        match crate::destinations::Table::read(
            &paths.config,
            &paths.projects,
            &paths.data,
            self.contexts(),
            &self.destination_windows,
        ) {
            Ok(mut table) => {
                table.add_menu();
                table.update_workspaces(&self.layout.snapshot());
                self.destinations = table;
            }
            Err(error) => {
                tracing::warn!(%error, "destination reload failed");
                self.show_notice("DESTINATION CONFIG ERROR");
            }
        }
        self.refresh_speech_vocabulary();
    }

    fn destination_vocabulary(&self) -> Vec<String> {
        let mut words = self.destinations.vocabulary_for(self.speech_context.as_deref());
        let contexts: Vec<_> = self
            .contexts()
            .iter()
            .filter(|context| {
                context.app_id() == "*" || Some(context.app_id()) == self.speech_context.as_deref()
            })
            .cloned()
            .collect();
        let project_names = crate::speech_vocabulary::directory_names(&self.destination_paths.projects);
        words.extend(
            crate::speech_vocabulary::load(&contexts)
                .into_iter()
                .filter(|word| !project_names.contains(word)),
        );
        crate::speech_vocabulary::bounded(words)
    }

    fn refresh_speech_vocabulary(&mut self) {
        if self.speech_process.is_some() {
            let words = self.destination_vocabulary();
            if let Some(process) = &mut self.speech_process
                && let Err(error) = process.set_vocabulary(&words)
            {
                tracing::warn!(%error, "speech vocabulary unavailable");
            }
        }
    }

    #[must_use]
    pub fn speech_ready(&self) -> bool {
        self.speech_process
            .as_ref()
            .is_some_and(crate::speech::SpeechProcess::is_ready)
    }

    #[must_use]
    pub fn speech_notice_visible(&self) -> bool {
        self.notice_until.is_some_and(|until| self.clock_now() < until)
    }

    #[must_use]
    pub fn visible_notice_text(&self) -> Option<String> {
        self.visible_notice().map(|(_, text)| text)
    }

    #[must_use]
    pub fn visible_notice(&self) -> Option<(tehuti_core::overlay::Overlay, String)> {
        tehuti_core::overlay::STACK
            .into_iter()
            .find_map(|layer| self.overlay_notice(layer).map(|text| (layer, text)))
    }

    fn overlay_notice(&self, layer: tehuti_core::overlay::Overlay) -> Option<String> {
        use tehuti_core::overlay::Overlay;
        match layer {
            Overlay::SystemMenu => self.dispatch.core().menu().map(tehuti_core::menu::Menu::text),
            Overlay::Secret => self.secret_confirmation_text(),
            Overlay::SecretUnavailable => self
                .secret_notice_until
                .is_some_and(|until| self.clock_now() < until)
                .then(|| "SECRET DELIVERY UNAVAILABLE".into()),
            Overlay::Notice => self.speech_notice_visible().then(|| {
                self.notice_text
                    .clone()
                    .unwrap_or_else(|| "NO SPEECH MAPPING".into())
            }),
            Overlay::MouseNotice => self.mouse_mode.then(|| "MOUSE MODE".into()),
            Overlay::Cursor
            | Overlay::Settings
            | Overlay::ChoiceList
            | Overlay::Bar
            | Overlay::Keyboard
            | Overlay::Mouse
            | Overlay::Application => None,
        }
    }

    pub fn speech_input(&mut self, input: tehuti_core::speech::SpeechInput) {
        let _rejection = self.dispatch(DispatchInput::Speech(input));
    }

    /// Routes a typed phrase through the same event path as an admitted transcript.
    pub fn say(&mut self, text: tehuti_protocol::Text) {
        tracing::info!(text = text.as_str(), "speech transcript received");
        self.handle(&Event::Speech { text });
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
        self.keep_invitation();
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
    #[must_use]
    pub fn pad_passthrough(&self) -> bool {
        self.dispatch.core().passthrough()
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
        u32::try_from(self.clock_elapsed().as_millis()).unwrap_or(u32::MAX)
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
        if !matches!(self.driver_clock, crate::driver::Clock::Live) {
            return core::time::Duration::ZERO;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.previous_tick);
        self.previous_tick = now;
        elapsed
    }

    /// Runs one event through the core dispatch step and carries out its effects.
    pub fn handle(&mut self, event: &Event) {
        if let Event::Speech { text } = event
            && !self.dispatch.keyboard().is_open()
            && !self.dispatch.keyboard().password_field_active()
            && let Some(name) = text
                .as_str()
                .get(..13)
                .filter(|prefix| prefix.eq_ignore_ascii_case("save this as "))
                .and_then(|_| text.as_str().get(13..))
        {
            self.save_destination(name.trim().trim_end_matches(['.', '!', '?']));
            return;
        }
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
        self.after_event(event);
    }

    fn after_event(&mut self, event: &Event) {
        if let Event::Focus { app_id, .. } = event {
            self.speech_context.clone_from(app_id);
            self.refresh_speech_vocabulary();
        }
        self.keep_invitation();
    }

    /// Shows the session-start invitation whenever nothing is open; the session enables it.
    pub const fn enable_invitation(&mut self) {
        self.invitation = Invitation::Ready;
    }

    /// With nothing open, the invitation is the first thing on screen; a window closes it.
    fn keep_invitation(&mut self) {
        if self.invitation == Invitation::Off {
            return;
        }
        if !self.layout_windows.is_empty() || self.speech_context.is_some() {
            self.invitation = Invitation::Ready;
            let _rejection = self.dispatch(DispatchInput::CloseInvitation);
            return;
        }
        let waiting = matches!(
            self.invitation,
            Invitation::Launching(since) if since.elapsed() < core::time::Duration::from_secs(10)
        );
        if waiting || self.choice_list().is_some() || self.dispatch.keyboard().is_open() {
            return;
        }
        let button = (!self.controller_names.is_empty()).then(|| match self.pad_family() {
            tehuti_core::pad::PadFamily::Letters => "Share",
            tehuti_core::pad::PadFamily::Symbols => "Create",
            tehuti_core::pad::PadFamily::Nintendo => "Capture",
        });
        let invitation = tehuti_core::launcher::Launcher::invitation(button);
        self.launcher_routes.clear();
        let _rejection = self.dispatch(DispatchInput::Launcher(invitation));
    }

    /// Feeds one input and every reply it asks for until its frame finishes.
    pub(crate) fn dispatch(&mut self, input: DispatchInput) -> Option<tehuti_core::plugin::Rejection> {
        let elapsed = dispatch_elapsed(&input);
        self.advance_clocks(elapsed);
        let level = self.dispatch.core().level();
        let depth = self.dispatch.busy();
        let mut input = input;
        let mut rejection = None;
        loop {
            let previous = self
                .dispatch
                .keyboard()
                .application_anchor()
                .map(|anchor| (self.dispatch.keyboard().caret(), anchor));
            let (next, effects) = tehuti_core::dispatch::step(core::mem::take(&mut self.dispatch), input);
            self.dispatch = next;
            if let Some(previous) = previous {
                self.sync_keyboard_edit(previous);
            }
            let mut reply = DispatchInput::Resume;
            for effect in effects {
                if let DispatchEffect::PluginRejected(rejected) = effect {
                    rejection = Some(rejected);
                } else if let Some(answer) = self.run_effect(effect) {
                    reply = answer;
                }
            }
            if self.dispatch.busy() <= depth {
                self.finish_dispatch_view(level);
                return rejection;
            }
            input = reply;
        }
    }

    fn finish_dispatch_view(&mut self, previous: tehuti_core::Level) {
        crate::wallpaper::set_focus(self.dispatch.desktop_focus());
        if previous != self.dispatch.core().level() {
            self.dim_elapsed = core::time::Duration::ZERO;
        }
        self.update_window_dimming();
        self.reserve_keyboard_height();
    }

    pub(crate) fn set_keyboard_surrounding(&mut self, surrounding: Option<(String, usize, usize)>) {
        self.dispatch.set_keyboard_surrounding(surrounding);
    }

    fn sync_keyboard_edit(&mut self, previous: (usize, usize)) {
        use tehuti_core::keyboard::{Chord, ChordKey, Modifiers};
        let edits = self.dispatch.take_keyboard_edits();
        let edited = !edits.is_empty();
        let mut position = previous.0;
        for edit in edits {
            self.text_input.edit_keyboard(&edit);
            position = edit.caret;
        }
        let current = self.dispatch.keyboard();
        if !current.synced() || !current.is_open() {
            return;
        }
        let selection = previous.0.min(previous.1)..previous.0.max(previous.1);
        let target = current.caret();
        let key = if target < position {
            tehuti_protocol::KeyName::Left
        } else {
            tehuti_protocol::KeyName::Right
        };
        if !edited && previous.0 == target {
            return;
        }
        if !edited && !selection.is_empty() {
            position = if target < position {
                selection.start
            } else {
                selection.end
            };
            self.tap_chord(Chord {
                key: ChordKey::Named(key),
                modifiers: Modifiers::default(),
            });
        }
        let count = self
            .dispatch
            .keyboard()
            .draft()
            .get(target.min(position)..target.max(position))
            .map_or(0, |text| text.chars().count());
        self.dispatch.keyboard_edit_applied();
        for _ in 0..count {
            self.tap_chord(Chord {
                key: ChordKey::Named(key),
                modifiers: Modifiers::default(),
            });
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
                    dimmed_brightness(strength, progress)
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

    pub fn open_choice_list(&mut self, slots: tehuti_core::choice_list::Slots) {
        let _rejection = self.dispatch(DispatchInput::ChoiceList(slots));
    }

    #[must_use]
    pub fn choice_list(&self) -> Option<&tehuti_core::choice_list::List> {
        self.dispatch.choice_list()
    }

    #[must_use]
    pub const fn pad_family(&self) -> tehuti_core::pad::PadFamily {
        self.dispatch.core().pad_family()
    }

    pub fn change_display_scale(&mut self, scale: core::num::NonZeroU32) {
        let Ok(scale) = i32::try_from(scale.get()) else {
            return;
        };
        let Some(output) = self.output.clone() else { return };
        let Some(mode) = output.current_mode() else { return };
        crate::scale::apply(&output, mode, scale);
        self.fill_output();
    }

    fn persist_display_scale(&mut self, scale: core::num::NonZeroU32) {
        self.change_display_scale(scale);
        let Ok(scale) = i32::try_from(scale.get()) else {
            return;
        };
        let result = crate::display_mode_store::Store::from_env().and_then(|store| store.write_scale(scale));
        if let Err(error) = result {
            tracing::warn!(%error, "display scale persistence failed");
            self.show_notice("scale not saved");
        }
    }

    pub fn load_bar_font(&mut self, store: crate::display_mode_store::Store) {
        match store.read() {
            Ok(value) => {
                let font = match value.as_deref() {
                    None | Some("Liberation Mono") => tehuti_core::bar::Font::LiberationMono,
                    Some("Bitmap") => tehuti_core::bar::Font::Bitmap,
                    Some(_) => {
                        tracing::warn!("invalid saved bar font; using Liberation Mono");
                        tehuti_core::bar::Font::LiberationMono
                    }
                };
                let _rejection = self.dispatch(DispatchInput::BarFont(font));
            }
            Err(error) => tracing::warn!(%error, "bar font could not be read"),
        }
        self.bar_font_store = Some(store);
    }

    /// Restores the saved controller and microphone and saves later choices to `store`.
    pub fn load_input_choices(&mut self, store: crate::input_choice_store::Store) {
        let controller = store.controller().unwrap_or_else(|error| {
            tracing::warn!(%error, "saved controller unreadable; using every pad");
            tehuti_core::dispatch::ControllerChoice::Auto
        });
        let microphone = store.microphone().unwrap_or_else(|error| {
            tracing::warn!(%error, "saved microphone unreadable; using every microphone");
            tehuti_core::dispatch::MicrophoneChoice::All
        });
        self.controller_choice = controller.clone();
        self.input_choices = Some(store);
        let _rejection = self.dispatch(DispatchInput::InputChoices {
            controller,
            microphone,
        });
    }

    /// Records the connected pad names; the settings view lists them.
    pub fn configure_controllers(&mut self, controllers: Vec<tehuti_core::dispatch::ControllerInfo>) {
        if controllers == self.settings().input.controllers {
            return;
        }
        let names: Vec<_> = controllers
            .iter()
            .map(|controller| controller.name.clone())
            .collect();
        self.controller_names = names;
        let _rejection = self.dispatch(DispatchInput::Controllers(controllers));
    }

    /// Whether input from the pad named `name` drives Tehuti.
    #[must_use]
    pub fn controller_chosen(&self, name: &str) -> bool {
        crate::input::chosen(&self.controller_choice, &self.controller_names, name)
    }

    fn choose_controller(&mut self, choice: tehuti_core::dispatch::ControllerChoice, persist: bool) {
        tracing::info!(?choice, persist, "controller choice");
        if persist {
            self.save_input_choice(|store| store.set_controller(&choice));
        }
        self.controller_choice = choice;
    }

    fn choose_microphone(&mut self, choice: &tehuti_core::dispatch::MicrophoneChoice) {
        tracing::info!(?choice, "microphone choice");
        self.save_input_choice(|store| store.set_microphone(choice));
    }

    fn save_input_choice(
        &mut self,
        result: impl FnOnce(&crate::input_choice_store::Store) -> std::io::Result<()>,
    ) {
        if let Some(Err(error)) = self.input_choices.as_ref().map(result) {
            tracing::warn!(%error, "input choice persistence failed");
            self.show_notice("CHOICE NOT SAVED");
        }
    }

    fn poll_microphones(&mut self) {
        let Some(listing) = &self.microphone_listing else {
            return;
        };
        match listing.try_recv() {
            Ok(microphones) => {
                self.microphone_listing = None;
                let _rejection = self.dispatch(DispatchInput::Microphones(microphones));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.microphone_listing = None,
        }
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

    #[expect(clippy::too_many_lines, reason = "effect routing is centralized here")]
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
                if matches!(self.driver_clock, crate::driver::Clock::Synthetic) {
                    false
                } else {
                    self.handle_secret_confirmation_button(state)
                },
            )),
            DispatchEffect::SpeechCommand(command) => {
                Some(DispatchInput::SpeechSent(self.send_speech_command(&command)))
            }
            DispatchEffect::PollSpeech(elapsed) => {
                Some(DispatchInput::SpeechPolled(self.poll_speech(elapsed)))
            }
            DispatchEffect::UnlockWallpapers => {
                match crate::wallpaper::unlock() {
                    Ok(_) => self.show_notice("WALLPAPERS UNLOCKED"),
                    Err(error) => {
                        tracing::warn!(%error, "wallpaper unlock failed");
                        self.show_notice("UNLOCK NOT SAVED");
                    }
                }
                let (names, locked, current) = crate::wallpaper::choices();
                let _rejection = self.dispatch(DispatchInput::Wallpapers {
                    names,
                    locked,
                    current,
                });
                None
            }
            effect @ (DispatchEffect::ChooseLauncher(_)
            | DispatchEffect::OpenLauncher(_)
            | DispatchEffect::SettingsMode(_)
            | DispatchEffect::PersistMode(_)
            | DispatchEffect::SettingsBarFont(_)
            | DispatchEffect::SettingsScale(_)
            | DispatchEffect::WallpaperPreview(_)
            | DispatchEffect::WallpaperSave(_)
            | DispatchEffect::SettingsController(_)
            | DispatchEffect::PersistController(_)
            | DispatchEffect::SettingsMicrophone(_)
            | DispatchEffect::ListMicrophones
            | DispatchEffect::PasswordSpeechIgnored
            | DispatchEffect::RunMenuCommand(_)
            | DispatchEffect::MovePointer { .. }
            | DispatchEffect::Scroll { .. }
            | DispatchEffect::Click { .. }
            | DispatchEffect::MouseMode(_)
            | DispatchEffect::PollSources
            | DispatchEffect::Chord(_)
            | DispatchEffect::ConfirmPressed(_)
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
        if command == tehuti_core::menu::Command::Screenshot {
            match crate::screenshot::save(self) {
                Ok(path) => self.show_notice(&format!("SCREENSHOT SAVED: {}", path.display())),
                Err(error) => self.show_notice(&format!("SCREENSHOT FAILED: {error}")),
            }
            return;
        }
        if command == tehuti_core::menu::Command::Restart {
            self.speech_input(tehuti_core::speech::SpeechInput::Cancel);
            self.restart_requested = true;
            self.quit_requested = true;
            return;
        }
        if let Some(status) = self.menu_config.session_exit_status(command) {
            self.speech_input(tehuti_core::speech::SpeechInput::Cancel);
            self.session_exit_status = Some(status);
            self.quit_requested = true;
            return;
        }
        let result = self
            .launch_socket
            .as_deref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotConnected, "no compositor socket"))
            .and_then(|socket| crate::system_menu::execute(&self.menu_config, command, socket));
        if let Err(error) = result {
            tracing::warn!(?command, %error, "system menu command failed");
            self.show_notice(&format!("MENU COMMAND UNAVAILABLE: {error}"));
        }
    }

    fn open_launcher(&mut self, request: &tehuti_core::unit_verbs::LauncherRequest) {
        let focused = self.speech_context.as_deref();
        let (destinations, routes) =
            crate::launcher::destinations(self.destinations.entries(), request.kind, focused);
        self.launcher_routes = routes;
        let mut launcher = tehuti_core::launcher::Launcher::new(&request.query, destinations, None);
        let contexts = self.dispatch.core().contexts();
        let active = contexts
            .iter()
            .find(|context| Some(context.app_id()) == focused)
            .map(tehuti_protocol::Context::launcher);
        let global = contexts
            .iter()
            .find(|context| context.app_id() == "*")
            .map(tehuti_protocol::Context::launcher);
        let enabled = active
            .and_then(|settings| settings.auto_pick)
            .or_else(|| global.and_then(|settings| settings.auto_pick))
            .unwrap_or(true);
        let delay = active
            .and_then(|settings| settings.delay_ms)
            .or_else(|| global.and_then(|settings| settings.delay_ms))
            .map_or(5000, core::num::NonZeroU32::get);
        launcher.configure(enabled, core::time::Duration::from_millis(u64::from(delay)));
        let verb = match request.kind {
            tehuti_core::unit_verbs::DestinationKind::Application => "Open",
            tehuti_core::unit_verbs::DestinationKind::Project => "Work on",
            tehuti_core::unit_verbs::DestinationKind::Creation => "New",
        };
        let level = match self.dispatch.core().level() {
            tehuti_core::Level::App => "App",
            tehuti_core::Level::Columns => "Columns",
        };
        launcher.set_scope(verb, level, focused);
        tracing::info!(query = request.query, rows = ?launcher.list().candidates(), "launcher local ranking");
        let jev = active
            .and_then(|settings| settings.jev)
            .or_else(|| global.and_then(|settings| settings.jev))
            .unwrap_or(false);
        let key_present = std::env::var_os("TYPESAFE_API_KEY").is_some_and(|key| !key.is_empty());
        if let Some(jev_request) = launcher.jev_request(jev, key_present, level, focused) {
            // The network client is not wired yet; the local ranking stands.
            tracing::info!(?jev_request, "launcher Jev request skipped: no client");
        }
        let _rejection = self.dispatch(DispatchInput::Launcher(launcher));
    }

    fn follow_launcher_route(&mut self, route: crate::destinations::Route) {
        use crate::destinations::Target;
        for step in route.steps {
            match step.target {
                Target::Workspace(identity) => {
                    if let Err(error) = self
                        .layout
                        .focus_workspace_id(tehuti_core::layout::WorkspaceId(identity))
                    {
                        tracing::warn!(%error, "launcher workspace focus failed");
                    }
                    self.place_layout();
                }
                Target::Menu(key) => {
                    if let Some(command) = tehuti_core::menu::ITEMS
                        .into_iter()
                        .find(|command| command.key() == key)
                    {
                        let _rejection = self.dispatch(DispatchInput::Menu(command));
                    }
                }
                Target::Desktop(entry) => {
                    if let Ok(entry) = tehuti_protocol::Text::new(entry) {
                        self.launch_application(&entry);
                    }
                }
                Target::Action(action) => {
                    self.apply(&action);
                }
                Target::Window(identity) => {
                    if let Err(error) = self.layout.focus_window(tehuti_core::layout::WindowId(identity)) {
                        tracing::warn!(%error, "launcher focus failed");
                    }
                    self.place_layout();
                }
                Target::Application(app_id) => {
                    if let Some(window) = self
                        .destination_windows
                        .iter()
                        .find(|window| window.app_id == app_id)
                    {
                        let identity = window.identity;
                        if let Err(error) = self.layout.focus_window(tehuti_core::layout::WindowId(identity))
                        {
                            tracing::warn!(%error, "launcher focus failed");
                        }
                        self.place_layout();
                    }
                }
                Target::Url(url) => self.launch_url(&url),
            }
        }
    }

    fn launch_url(&mut self, url: &str) {
        let Some(socket) = self.launch_socket.as_deref() else {
            self.show_launch_failure();
            return;
        };
        if self.launchers.len() >= 16 {
            self.show_launch_failure();
            return;
        }
        match crate::client_env::command(std::ffi::OsStr::new("xdg-open"), socket)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => self.launchers.push(child),
            Err(error) => {
                tracing::warn!(%error, "launcher URL failed");
                self.show_launch_failure();
            }
        }
    }

    /// Runs an effect that needs no reply; `run_effect` answers the queries.
    #[expect(clippy::too_many_lines, reason = "one flat table of every effect")]
    fn carry_out(&mut self, effect: DispatchEffect) {
        use tehuti_core::speech::SpeechFailure;
        match effect {
            DispatchEffect::OpenLauncher(request) => {
                self.open_launcher(&request);
            }
            DispatchEffect::ChooseLauncher(index) => {
                if let Some(route) = self.launcher_routes.get(index).cloned() {
                    if self.invitation != Invitation::Off && self.layout_windows.is_empty() {
                        self.invitation = Invitation::Launching(Instant::now());
                    }
                    self.follow_launcher_route(route);
                }
            }
            DispatchEffect::SettingsScale(scale) => self.persist_display_scale(scale),
            DispatchEffect::WallpaperPreview(index) => {
                if let Err(error) = crate::wallpaper::preview(index) {
                    tracing::warn!(%error, "wallpaper preview failed");
                }
            }
            DispatchEffect::WallpaperSave(index) => match crate::wallpaper::save(index) {
                Ok(()) => {
                    let _rejection = self.dispatch(DispatchInput::WallpaperSaved(index));
                }
                Err(error) => {
                    tracing::warn!(%error, "wallpaper save failed");
                    self.show_notice("WALLPAPER NOT SAVED");
                }
            },
            DispatchEffect::SettingsBarFont(font) => {
                let result = self
                    .bar_font_store
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("bar font store unavailable"))
                    .and_then(|store| store.write(font.label()));
                match result {
                    Ok(()) => {
                        let _rejection = self.dispatch(DispatchInput::BarFont(font));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "bar font was not saved");
                        self.show_notice("BAR FONT WAS NOT SAVED");
                    }
                }
            }
            DispatchEffect::SettingsMode(index) => self.queue_settings(index, false),
            DispatchEffect::PersistMode(index) => self.queue_settings(index, true),
            DispatchEffect::SettingsController(choice) => self.choose_controller(choice, false),
            DispatchEffect::PersistController(choice) => self.choose_controller(choice, true),
            DispatchEffect::SettingsMicrophone(choice) => self.choose_microphone(&choice),
            DispatchEffect::ListMicrophones => {
                self.microphone_listing = Some(crate::input_choice_store::list_microphones());
            }
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
                self.sync_secret_prompt();
                self.poll_secret_lookup();
                self.poll_launchers();
                self.desktop_entries.poll();
                self.poll_microphones();
            }
            DispatchEffect::ConfirmPressed(pressed) => self.secret_confirmation.confirm_pressed = pressed,
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
            | DispatchEffect::ConfirmSecret(_)
            | DispatchEffect::SpeechCommand(_)
            | DispatchEffect::PollSpeech(_)
            | DispatchEffect::UnlockWallpapers => {}
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
            DispatchEffect::SpeechRouted {
                text,
                first,
                plugin,
                fuzzy,
            } => {
                if let Some(matched) = fuzzy {
                    tracing::info!(
                        text = text.as_str(),
                        phrase = matched.phrase,
                        score = %thousandths(matched.score),
                        "speech fuzzy matched"
                    );
                }
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
        self.sync_secret_prompt();
    }

    #[must_use]
    pub const fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    #[must_use]
    pub const fn log_out_requested(&self) -> bool {
        matches!(self.session_exit_status, Some(21))
    }

    #[must_use]
    pub const fn restart_requested(&self) -> bool {
        self.restart_requested
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
                self.clock_elapsed(),
                core::time::Duration::from_secs(20),
            )
            .is_some();
        if requested {
            self.secret_confirmation_label = Some((entry_label, sanitize_confirmation_label(&target, 29)));
            self.secret_confirmation.frame_submitted = false;
            self.secret_confirmation.armed = false;
        }
        self.sync_secret_prompt();
        requested
    }

    fn sync_secret_prompt(&mut self) {
        let visible = self.secret_confirmation_text().is_some();
        if visible != self.dispatch.secret_prompt_visible() {
            let _rejection = self.dispatch(DispatchInput::SecretPrompt(visible));
        }
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
                let _pending = self.secret_gate.pending(self.clock_elapsed(), destination)?;
                Some(format!("CONFIRM SECRET: {entry}\nTARGET: {target}"))
            })
    }

    /// Records that a damaged frame containing the confirmation was submitted.
    pub fn note_secret_confirmation_frame_submitted(&mut self) {
        if self.secret_confirmation_text().is_some() {
            self.secret_confirmation.frame_submitted = true;
            self.secret_confirmation.armed = !self.secret_confirmation.confirm_pressed;
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
            .pending(self.clock_elapsed(), destination)
            .is_none()
        {
            return false;
        }
        match state {
            tehuti_protocol::ButtonState::Released => {
                self.secret_confirmation.confirm_pressed = false;
                if self.secret_confirmation.frame_submitted {
                    self.secret_confirmation.armed = true;
                }
            }
            tehuti_protocol::ButtonState::Pressed if self.secret_confirmation.armed => {
                self.secret_confirmation.confirm_pressed = true;
                self.secret_confirmation.armed = false;
                let _consumed = self.confirm_pending_secret();
            }
            tehuti_protocol::ButtonState::Pressed => self.secret_confirmation.confirm_pressed = true,
        }
        true
    }

    fn confirm_pending_secret(&mut self) -> bool {
        let Some(destination) = self.current_secret_destination() else {
            return false;
        };
        let now = self.clock_elapsed();
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
        self.sync_secret_prompt();
        let Some(mut provider) = self.secret_provider.take() else {
            self.secret_notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
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
            self.secret_notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
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
                self.secret_notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
                return;
            }
        };
        let Some(lookup) = lookup_result else {
            return;
        };
        self.secret_lookup = None;
        self.secret_provider = Some(lookup.provider);
        let Some(secret) = lookup.value.ok() else {
            self.secret_notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
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
            self.secret_notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
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
            Action::KeyDown { key } => self.press_held_key(*key),
            Action::KeyUp { key } => self.release_held_key(*key),
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
                    self.secret_notice_until =
                        self.clock_now().checked_add(core::time::Duration::from_secs(3));
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
            Action::ToggleTicker => self.dispatch.core_mut().toggle_ticker(),
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
        self.notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
    }

    fn tap(&mut self, key: KeyName) {
        let evdev_code = key.evdev_code();
        let keycode = Keycode::new(evdev_code.saturating_add(XKB_KEYCODE_OFFSET));
        self.tap_keycode(keycode);
    }

    fn press_held_key(&mut self, key: KeyName) {
        if self.held_pad_keys.iter().any(|(held, _)| *held == key) {
            return;
        }
        let Some(keycode) = self.resolve_keysym(key.keysym_name()) else {
            tracing::warn!(?key, "held key unavailable in active keymap");
            return;
        };
        self.held_pad_keys.push((key, keycode));
        self.send_keycode(keycode, smithay::backend::input::KeyState::Pressed);
        let (app_id, title) = self.focused_window_label();
        tracing::info!(?key, ?app_id, ?title, "key held in focused window");
    }

    /// Releases on the keycode the key went down on, even if the keymap changed since.
    fn release_held_key(&mut self, key: KeyName) {
        let Some(index) = self.held_pad_keys.iter().position(|(held, _)| *held == key) else {
            return;
        };
        let (_, keycode) = self.held_pad_keys.remove(index);
        self.send_keycode(keycode, smithay::backend::input::KeyState::Released);
        tracing::info!(?key, "held key released");
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

    /// Releases every held host and pad key, so a key held when the host window loses focus or the seat pauses does not stick.
    pub fn release_host_keys(&mut self) {
        self.held_pad_keys.clear();
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
        u32::try_from(self.clock_elapsed().as_millis()).unwrap_or(u32::MAX)
    }

    fn apply_layout(&mut self, action: tehuti_protocol::LayoutAction) {
        self.apply_layout_command(tehuti_core::layout::command_for(action));
    }

    /// Applies a tiling command and places the mapped windows.
    pub fn apply_layout_command(&mut self, command: tehuti_core::layout::Command) {
        self.layout.apply(command);
        self.place_layout();
    }

    /// The next opening slot while the launcher is visible.
    #[must_use]
    pub fn launcher_slot(&self) -> Option<tehuti_core::layout::Rect> {
        let _launcher = self.dispatch.launcher()?;
        self.layout
            .preview_window(tehuti_core::layout::WindowRules::default())
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
        let bounds = Rectangle::new(location.into(), window.client_geometry().size);
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
                let title = self
                    .destinations
                    .name_for(&crate::destinations::Target::Window(tile.window.0))
                    .map(str::to_owned)
                    .or_else(|| {
                        let toplevel = window.toplevel()?;
                        smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
                            let attributes = states
                                .data_map
                                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()?;
                            let attributes = attributes
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            Some(self.dispatch.core().bar().window_label(
                                self.dispatch.core().label(attributes.app_id.as_deref()),
                                attributes.title.as_deref(),
                                attributes.app_id.as_deref(),
                            ))
                        })
                    })?;
                Some((title, snapshot.focused_window == Some(tile.window)))
            })
            .collect();
        let mut bar = self.dispatch.core().bar().clone().with_windows(windows);
        if let Some(identity) = snapshot.focused_window
            && let Some(name) = self
                .destinations
                .name_for(&crate::destinations::Target::Window(identity.0))
        {
            bar = bar.with_destination(name.into());
        }
        if snapshot.focused_window.is_none()
            && let Some(output) = snapshot
                .outputs
                .iter()
                .find(|output| Some(output.output) == snapshot.focused_output)
            && let Some(workspace) = output.workspaces.get(output.active_workspace)
            && let Some(name) = self
                .destinations
                .name_for(&crate::destinations::Target::Workspace(workspace.workspace.0))
        {
            bar = bar.with_destination(name.into());
        }
        bar.font = self.settings().bar_font;
        bar
    }

    /// Updates the layout to the output's logical size and configures its windows.
    #[expect(
        clippy::float_arithmetic,
        reason = "reserve the bar in logical output coordinates"
    )]
    pub fn fill_output(&mut self) {
        use tehuti_core::layout::{OutputGeometry, OutputId, Rect};
        if let Some(output) = self.output.as_ref()
            && let Some(scale) =
                core::num::NonZeroU32::new(output.current_scale().integer_scale().unsigned_abs())
        {
            let _rejection = self.dispatch(DispatchInput::DisplayScale(scale));
        }
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
            window.set_size(
                smithay::utils::Size::<f64, smithay::utils::Logical>::from((
                    tile.requested_client.width,
                    tile.requested_client.height,
                ))
                .to_i32_round(),
            );
            let client_size = if window.mapped() {
                window.client_geometry().size
            } else {
                window.layout_size()
            };
            let placement = tile.place_client(
                tehuti_core::layout::Size {
                    width: f64::from(client_size.w),
                    height: f64::from(client_size.h),
                },
                border,
            );
            self.space.map_element(
                window.clone(),
                smithay::utils::Point::<f64, smithay::utils::Logical>::from((placement.x, placement.y))
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
                Self::configure_layout_window(toplevel, tile, snapshot.focused_window == Some(*identity));
            }
        }
        self.sync_window_focus();
        self.update_window_dimming();
    }

    fn configure_layout_window(
        toplevel: &ToplevelSurface,
        tile: &tehuti_core::layout::TileView,
        focused: bool,
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
        if toplevel.is_initial_configure_sent() {
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
        if self.invitation != Invitation::Off {
            self.invitation = Invitation::Ready;
        }
        self.notice_text = Some("APPLICATION UNAVAILABLE".into());
        self.notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
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

    fn save_destination(&mut self, name: &str) {
        let (app_id, title) = self.focused_window_label();
        let Some(app_id) = app_id else {
            self.show_notice("NO FOCUSED WINDOW");
            return;
        };
        let window = crate::destinations::Window {
            identity: 0,
            app_id,
            title: title.unwrap_or_default(),
            chip: String::new(),
        };
        match crate::destinations::save(&self.destination_paths.config, name, &window) {
            Ok(()) => {
                self.rebuild_destinations();
                self.show_notice("DESTINATION SAVED");
            }
            Err(error) => {
                tracing::warn!(%error, "destination save failed");
                self.show_notice("DESTINATION SAVE FAILED");
            }
        }
    }

    fn sync_destination_windows(&mut self) {
        self.destinations.update_workspaces(&self.layout.snapshot());
        let windows: Vec<_> = self
            .layout_windows
            .iter()
            .filter_map(|(identity, window)| {
                let toplevel = window.toplevel()?;
                smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
                    let attributes = states
                        .data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()?;
                    let attributes = attributes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let chip = self.dispatch.core().bar().window_label(
                        self.dispatch.core().label(attributes.app_id.as_deref()),
                        attributes.title.as_deref(),
                        attributes.app_id.as_deref(),
                    );
                    Some(crate::destinations::Window {
                        identity: identity.0,
                        app_id: attributes.app_id.clone().unwrap_or_default(),
                        title: attributes.title.clone().unwrap_or_default(),
                        chip,
                    })
                })
            })
            .collect();
        if windows != self.destination_windows {
            self.destinations.update_windows(&windows);
            self.destination_windows = windows;
            self.refresh_speech_vocabulary();
        }
    }

    fn sync_window_focus(&mut self) {
        self.sync_destination_windows();
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
        } else if self.window_focus.take().is_some() {
            self.dispatch.core_mut().set_context_ancestors(None, Vec::new());
            self.handle(&Event::Focus {
                app_id: None,
                title: String::new(),
            });
        }
    }

    fn update_focus(&mut self, surface: &ToplevelSurface) {
        if let Some(event) = self.focus_event(surface) {
            if let Event::Focus { app_id, .. } = &event {
                let focus = (surface.wl_surface().clone(), app_id.clone());
                if self.window_focus.as_ref() == Some(&focus) {
                    return;
                }
                self.window_focus = Some(focus);
                let windows = self
                    .layout_windows
                    .iter()
                    .filter_map(|(_, window)| window.toplevel().map(|toplevel| toplevel.wl_surface().clone()))
                    .collect::<Vec<_>>();
                let ancestors =
                    crate::window_context::ancestors(surface.wl_surface(), &windows, &self.display);
                self.dispatch
                    .core_mut()
                    .set_context_ancestors(app_id.clone(), ancestors);
            }
            self.handle(&event);
        }
    }

    fn focus_event(&self, surface: &ToplevelSurface) -> Option<Event> {
        if self.keyboard.current_focus().as_ref() != Some(surface.wl_surface()) {
            return None;
        }
        smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
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
        })
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
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        let previous = self
            .window_for_root(&root)
            .map(|window| (window.client_geometry(), window.mapped()));
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
        if self.window_for_root(&root).is_none()
            && let Some(toplevel) = self
                .xdg_shell_state
                .toplevel_surfaces()
                .iter()
                .find(|toplevel| toplevel.wl_surface() == &root)
                .cloned()
        {
            self.map_toplevel(toplevel);
        }
        if let Some(window) = self.window_for_root(&root).cloned() {
            window.on_commit();
            if previous != Some((window.client_geometry(), window.mapped())) {
                self.place_layout();
            }
            if let Some(toplevel) = window.toplevel()
                && !toplevel.is_initial_configure_sent()
            {
                let _serial = toplevel.send_configure();
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

impl Compositor {
    fn map_toplevel(&mut self, surface: ToplevelSurface) {
        let _effect = self
            .windows
            .step(tehuti_core::windows::Input::Map(surface.wl_surface().clone()));
        self.fill_output();
        let identity = tehuti_core::layout::WindowId(self.next_window);
        self.next_window = self.next_window.saturating_add(1);
        let app_id = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|attributes| attributes.lock().ok()?.app_id.clone())
        });
        let windows = self
            .layout_windows
            .iter()
            .filter_map(|(_, window)| window.toplevel().map(|toplevel| toplevel.wl_surface().clone()))
            .collect::<Vec<_>>();
        let ancestors = crate::window_context::ancestors(surface.wl_surface(), &windows, &self.display);
        let context = app_id
            .as_deref()
            .into_iter()
            .chain(ancestors.iter().map(String::as_str))
            .find_map(|app_id| {
                self.contexts()
                    .iter()
                    .find(|context| app_id != "*" && context.app_id() == app_id)
            });
        let rules = tehuti_core::layout::WindowRules {
            open_fullscreen: context.is_some_and(tehuti_protocol::Context::open_fullscreen),
            default_column_width: context
                .and_then(tehuti_protocol::Context::window_column_width)
                .map(|width| Some(tehuti_core::layout::PresetSize::Proportion(width))),
            default_window_height: context
                .and_then(tehuti_protocol::Context::window_height)
                .map(tehuti_core::layout::PresetSize::Proportion),
            ..Default::default()
        };
        match self.layout.add_window(identity, rules) {
            Ok(()) => {
                self.layout_windows
                    .push((identity, LayoutWindow::new(Window::new_wayland_window(surface))));
                self.place_layout();
            }
            Err(error) => tracing::warn!(%error, "window layout failed"),
        }
    }
}

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, _surface: ToplevelSurface) {}

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.sync_destination_windows();
        self.update_focus(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.sync_destination_windows();
        if let Some(Event::Focus { app_id, title }) = self.focus_event(&surface) {
            let _rejection = self.dispatch(DispatchInput::FocusTitle { app_id, title });
        }
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

impl Compositor {
    /// Applies the decorations setting. With [`Decorations::None`](crate::decorations::Decorations::None)
    /// it advertises `zxdg_decoration_manager_v1` and answers every toplevel with server-side mode,
    /// so clients drop their title bars; Tehuti draws none. With `Client` no global is advertised.
    pub fn set_decorations(&mut self, decorations: crate::decorations::Decorations) {
        if decorations == crate::decorations::Decorations::None && self.decoration_state.is_none() {
            self.decoration_state = Some(XdgDecorationState::new::<Self>(&self.display));
        }
    }
}

#[cfg(not(target_arch = "x86"))]
#[expect(
    clippy::float_arithmetic,
    reason = "linear colour fade over validated duration"
)]
fn dimmed_brightness(strength: f32, progress: f32) -> f32 {
    (1.0 - strength).mul_add(-progress, 1.0)
}

#[cfg(target_arch = "x86")]
#[expect(
    clippy::float_arithmetic,
    clippy::suboptimal_flops,
    reason = "32-bit x86 CPUs without FMA otherwise call software fmaf"
)]
fn dimmed_brightness(strength: f32, progress: f32) -> f32 {
    1.0 - (1.0 - strength) * progress
}

fn configure_server_side(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ServerSide));
    if toplevel.is_initial_configure_sent() {
        let _serial = toplevel.send_configure();
    }
}

impl XdgDecorationHandler for Compositor {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        configure_server_side(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        configure_server_side(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        configure_server_side(&toplevel);
    }
}

impl SeatHandler for Compositor {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        set_data_device_focus(
            &self.display,
            seat,
            focused.and_then(smithay::reexports::wayland_server::Resource::client),
        );
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
delegate_xdg_decoration!(Compositor);
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

/// A score in thousandths as a decimal fraction, such as `0.875`.
fn thousandths(score: u16) -> String {
    format!(
        "{}.{:03}",
        score.checked_div(1000).unwrap_or_default(),
        score.checked_rem(1000).unwrap_or_default()
    )
}

fn log_transcript(session: u64, text: &tehuti_protocol::Text, admitted: bool) {
    tracing::info!(session, text = text.as_str(), "speech transcript received");
    if !admitted {
        tracing::info!("speech transcript dropped for a stale session");
    }
}

/// The session-start invitation's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invitation {
    /// Embedders and tests that never asked for it.
    Off,
    Ready,
    /// A pick from the invitation is starting; its window has a grace period to appear.
    Launching(Instant),
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
