//! Smithay state and protocol handlers for one output and its tiling layout.

#[path = "recording.rs"]
mod recording;

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
        | DispatchInput::Volume(_)
        | DispatchInput::Adjusters(_)
        | DispatchInput::Launcher(_)
        | DispatchInput::CloseInvitation
        | DispatchInput::ChoiceList(_)
        | DispatchInput::FocusTitle { .. }
        | DispatchInput::DisplayModes { .. }
        | DispatchInput::BarFont(_)
        | DispatchInput::DisplayScale(_)
        | DispatchInput::ModeApplied(_)
        | DispatchInput::ControllerBindings(_)
        | DispatchInput::ActiveController(_)
        | DispatchInput::Controllers(_)
        | DispatchInput::Microphones(_)
        | DispatchInput::InputChoices { .. }
        | DispatchInput::Speech(_)
        | DispatchInput::PluginReply { .. }
        | DispatchInput::PluginExited { .. }
        | DispatchInput::TextField { .. }
        | DispatchInput::ControllerDisconnected
        | DispatchInput::FocusedWindowClosed
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
    menu_failures: (std::sync::mpsc::Sender<String>, std::sync::mpsc::Receiver<String>),
    settings_display: crate::settings::Display,
    bar_font_store: Option<crate::display_mode_store::Store>,
    /// The pad that drives Tehuti; previews before confirmation too.
    controller_choice: tehuti_core::dispatch::ControllerChoice,
    controller_names: Vec<String>,
    input_choices: Option<crate::input_choice_store::Store>,
    volume: Option<crate::volume::Backend>,
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
    confirmation: crate::secret_confirmation::SecretConfirmation<WlSurface>,
    notice_until: Option<Instant>,
    taps: tehuti_core::tap_hold::TapHold<Keycode>,
    notice_text: Option<String>,
    recording_failure: Option<String>,
    workspace_feedback: crate::workspace_feedback::Feedback,
    launcher: crate::launcher::Session,
    /// Each installed context with its file, so a broken edit keeps the last good version.
    context_files: Vec<(std::path::PathBuf, tehuti_protocol::Context)>,
    speech_context: Option<String>,
    speech_title: String,
    launch_socket: Option<std::ffi::OsString>,
    placement: crate::window_placement::Placement<WlSurface, LayoutWindow>,
    surfaces: CompositorState,
    xdg_shell_state: XdgShellState,
    decoration_state: Option<XdgDecorationState>,
    shm_state: ShmState,
    _viewporter_state: ViewporterState,
    _fractional_scale_state: FractionalScaleManagerState,
    pub(crate) dmabuf: crate::rendering::DmabufSupport,
    recording: Option<recording::Session>,
    recording_runtime: Option<std::rc::Rc<dyn crate::recording_encoder::Runtime>>,
    pub(crate) capture_source: crate::screenshot::CaptureSource,
    _output_manager_state: OutputManagerState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    keyboard: KeyboardHandle<Self>,
    window_focus: Option<(WlSurface, Option<String>)>,
    /// The window focus last left for no window, until a window takes focus.
    unfocused_window: Option<WlSurface>,
    controller_session: tehuti_core::controller::State,
    seat: Seat<Self>,
    shortcut_keycodes: Vec<(Keycode, u32)>,
    text_keys: std::collections::HashMap<char, (Keycode, ModifiersState)>,
    keymap: String,
    pub(crate) text_input: crate::text_input::TextInput,
    keyboard_reserved: i32,
    /// Keyboard focus moved while smithay held the keyboard lock; its dispatch runs once the lock is released (#445).
    focus_change_dispatch: Option<DispatchInput>,
    /// Keys a held pad button keeps down, with the keycode they went down on.
    held_pad_keys: Vec<(KeyName, Keycode)>,
    /// Host Meta keys currently down, as evdev codes; Meta is the Guide button.
    meta_held: Vec<u32>,
    /// Host keys a Meta chord consumed, so their release is consumed too.
    consumed_host_keys: Vec<Keycode>,
    virtual_pad_buttons: Vec<tehuti_protocol::ButtonName>,
    virtual_pad_talk: Vec<tehuti_protocol::ButtonName>,
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
    /// The compiled map shared by Wayland and X11 clients.
    #[must_use]
    pub fn keymap(&self) -> &str {
        &self.keymap
    }

    /// Updates the client keymap and text-entry key lookup together.
    ///
    /// # Errors
    /// Smithay could not install the compiled map.
    pub fn set_keymap(&mut self, keymap: &xkb::Keymap) -> Result<(), smithay::input::keyboard::Error> {
        let serialized = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        self.keyboard
            .clone()
            .set_keymap_from_string(self, serialized.clone())?;
        self.text_keys = text_keys(keymap);
        self.shortcut_keycodes = (keymap.min_keycode().raw()..=keymap.max_keycode().raw())
            .map(|code| {
                let keycode = Keycode::new(code);
                (keycode, keymap.num_layouts_for_key(keycode))
            })
            .collect();
        self.keymap = serialized;
        Ok(())
    }

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
            menu_failures: std::sync::mpsc::channel(),
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
            volume: None,
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
            confirmation: crate::secret_confirmation::SecretConfirmation::new(),
            window_focus: None,
            unfocused_window: None,
            notice_until: None,
            taps: tehuti_core::tap_hold::TapHold::default(),
            notice_text: None,
            recording_failure: None,
            workspace_feedback: crate::workspace_feedback::Feedback::default(),
            launcher: crate::launcher::Session::new()?,
            context_files: Vec::new(),
            placement: crate::window_placement::Placement::new(),
            launch_socket: None,
            speech_context: None,
            speech_title: String::new(),
            surfaces: CompositorState::new_v6::<Self>(display),
            xdg_shell_state: XdgShellState::new::<Self>(display),
            decoration_state: None,
            shm_state: ShmState::new::<Self>(display, vec![]),
            _viewporter_state: ViewporterState::new::<Self>(display),
            _fractional_scale_state: FractionalScaleManagerState::new::<Self>(display),
            dmabuf: crate::rendering::DmabufSupport::default(),
            recording: None,
            recording_runtime: None,
            capture_source: crate::screenshot::CaptureSource::default(),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(display),
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(display),
            keyboard,
            controller_session: tehuti_core::controller::State::default(),
            seat,
            shortcut_keycodes: Vec::new(),
            text_keys,
            keymap: String::new(),
            text_input: crate::text_input::TextInput::new(display),
            keyboard_reserved: 0,
            focus_change_dispatch: None,
            held_pad_keys: Vec::new(),
            meta_held: Vec::new(),
            consumed_host_keys: Vec::new(),
            virtual_pad_buttons: Vec::new(),
            virtual_pad_talk: Vec::new(),
            display: display.clone(),
        };
        compositor.set_keymap(&keymap)?;
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
        compositor
            .dispatch
            .core_mut()
            .set_menu_hidden(compositor.menu_config.hidden());
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
        let focused = self.placement.focused_destination();
        let windows: Vec<_> = self.placement.placed(|_| 0.0).iter().map(|placed| {
            let window = placed.window;
            let (app_id, title) = window.toplevel().and_then(|surface| self.focus_event(surface)).and_then(|event| {
                if let Event::Focus { app_id, title } = event { Some((app_id, title)) } else { None }
            }).unwrap_or_default();
            let location = self.space.element_location(window).unwrap_or_default();
            let size = window.layout_size();
            json!({"id": placed.identity, "app_id": app_id, "title": title, "mapped": window.mapped(),
                "focused": placed.focused, "geometry": {"x": location.x, "y": location.y, "width": size.w, "height": size.h}})
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
        json!({"version": 1, "clock_ms": self.driver_elapsed.as_millis(), "windows": windows, "focus": focused,
            "level": format!("{:?}", self.dispatch.core().level()), "launcher": launcher,
            "overlays": {"keyboard": self.on_screen_keyboard().is_open(), "settings": self.settings().open, "menu": self.dispatch.core().menu().is_some()},
            "keyboard": self.on_screen_keyboard().is_open(), "notice": self.visible_notice_text(), "bar": bar,
            "menu_text": self.dispatch.core().menu().map(|menu| menu.clone().with_recording(self.recording_active()).text()),
            "notices": tehuti_core::overlay::STACK.into_iter().filter_map(|layer| self.overlay_notice(layer)).collect::<Vec<_>>(),
            "recording": self.recording_active()})
    }

    /// Installs contexts before accepting clients.
    pub fn set_contexts(&mut self, mut contexts: Vec<tehuti_protocol::Context>) {
        crate::speech_vocabulary::configure_projects(&mut contexts, &self.launcher.paths().projects);
        *self.dispatch.core_mut() = tehuti_core::State::with_contexts(contexts);
        self.dispatch
            .core_mut()
            .set_menu_entries(self.menu_config.labels());
        self.dispatch
            .core_mut()
            .set_menu_hidden(self.menu_config.hidden());
        self.window_focus = None;
        self.sync_window_focus();
        self.rebuild_destinations();
    }

    /// Installs contexts with their files before accepting clients.
    pub fn set_context_files(&mut self, files: Vec<(std::path::PathBuf, tehuti_protocol::Context)>) {
        self.set_contexts(files.iter().map(|(_, context)| context.clone()).collect());
        let skipped = files.iter().flat_map(|(path, context)| {
            context
                .skipped_phrases()
                .iter()
                .map(|message| crate::contexts::LoadError {
                    path: path.clone(),
                    message: message.clone(),
                })
        });
        let mut last = None;
        for warning in skipped {
            tracing::warn!(%warning, "speech phrase skipped");
            last = Some(warning);
        }
        if let Some(warning) = last {
            self.show_notice(&warning.phrase_notice());
        }
        self.context_files = files;
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
                crate::speech_vocabulary::configure_projects(&mut contexts, &self.launcher.paths().projects);
                tracing::info!(count = contexts.len(), "contexts reloaded");
                self.dispatch.core_mut().replace_contexts(contexts);
                self.sync_destination_windows();
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

    /// Applies a reload. A skipped file keeps its last good version; each skipped file
    /// and dropped phrase is logged, and the last of them is named in a notice.
    pub fn apply_reload(&mut self, reload: Result<crate::contexts::Reloaded, crate::contexts::LoadError>) {
        match reload {
            Ok(reloaded) => {
                let crate::contexts::Reloaded {
                    contexts,
                    paths,
                    skipped,
                    phrase_warnings,
                } = reloaded;
                let mut files: Vec<_> = paths.into_iter().zip(contexts).collect();
                for error in &skipped {
                    if let Some(kept) = self.context_files.iter().find(|(path, _)| *path == error.path) {
                        files.push(kept.clone());
                    }
                }
                files.sort_by(|left, right| left.0.cmp(&right.0));
                self.reload_contexts(Ok(files.iter().map(|(_, context)| context.clone()).collect()));
                self.context_files = files;
                for error in &skipped {
                    tracing::warn!(%error, "context skipped on reload; keeping its last good version");
                }
                for warning in &phrase_warnings {
                    tracing::warn!(%warning, "speech phrase skipped on reload");
                }
                if let Some(error) = skipped.last() {
                    self.show_notice(&error.notice());
                } else if let Some(warning) = phrase_warnings.last() {
                    self.show_notice(&warning.phrase_notice());
                }
            }
            Err(error) => self.reload_contexts(Err(error)),
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
        self.confirmation.set_provider(provider);
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
        self.launcher.configure(paths);
        self.rebuild_destinations();
    }

    /// Rebuilds the destination table from its discovery directories.
    pub fn refresh_destinations(&mut self) {
        self.rebuild_destinations();
    }

    /// The discovery directories the destination table reads.
    #[must_use]
    pub const fn destination_paths(&self) -> &crate::destinations::Paths {
        self.launcher.paths()
    }

    #[must_use]
    pub const fn destinations(&self) -> &crate::destinations::Table {
        self.launcher.table()
    }

    fn rebuild_destinations(&mut self) {
        match self.launcher.rebuild(
            self.dispatch.core().contexts(),
            &self.menu_config.hidden(),
            &self.placement.snapshot(),
        ) {
            Ok(app_names) => {
                self.dispatch.core_mut().set_app_names(app_names);
            }
            Err(error) => {
                tracing::warn!(%error, "destination reload failed");
                self.show_notice("DESTINATION CONFIG ERROR");
            }
        }
        self.refresh_speech_vocabulary();
    }

    fn destination_vocabulary(&self) -> Vec<String> {
        let mut words = self
            .launcher
            .table()
            .vocabulary_for(self.speech_context.as_deref());
        let own = self
            .speech_context
            .as_deref()
            .and_then(|app_id| tehuti_core::own_context(self.contexts(), app_id, &self.speech_title));
        let contexts: Vec<_> = self
            .contexts()
            .iter()
            .filter(|context| context.app_id() == "*" || own.is_some_and(|own| core::ptr::eq(own, *context)))
            .cloned()
            .collect();
        let project_names = crate::speech_vocabulary::directory_names(&self.launcher.paths().projects);
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
        self.recording_failure.is_some() || self.notice_until.is_some_and(|until| self.clock_now() < until)
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
            Overlay::SystemMenu => self
                .dispatch
                .core()
                .menu()
                .map(|menu| menu.clone().with_recording(self.recording_active()).text()),
            Overlay::Secret => self.secret_confirmation_text(),
            Overlay::SecretUnavailable => self
                .confirmation
                .unavailable(self.clock_elapsed())
                .then(|| "SECRET DELIVERY UNAVAILABLE".into()),
            Overlay::Notice => self.speech_notice_visible().then(|| {
                self.recording_failure
                    .clone()
                    .or_else(|| self.notice_text.clone())
                    .unwrap_or_else(|| "NO SPEECH MAPPING".into())
            }),
            Overlay::MouseNotice => self.mouse_mode.then(|| "MOUSE MODE".into()),
            Overlay::Adjuster
            | Overlay::Cursor
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
        tracing::info!(bytes = text.as_str().len(), "speech transcript received");
        self.handle(&Event::Speech { text });
    }

    pub(crate) fn set_pad_overrides(&mut self, plugin: usize, overrides: Vec<tehuti_protocol::PadOverride>) {
        if let Some(plugins) = &mut self.plugins {
            plugins.set_pad_overrides(plugin, overrides);
        }
    }

    /// Whether the focused context presents the virtual pad now.
    #[must_use]
    pub fn virtual_pad(&self) -> bool {
        self.pad_passthrough() && self.dispatch.core().virtual_pad()
    }

    /// Forgets held physical buttons when the virtual pad starts.
    pub fn reset_virtual_pad(&mut self) {
        self.virtual_pad_buttons.clear();
        self.virtual_pad_talk.clear();
        if let Some(plugins) = &mut self.plugins {
            plugins.reset_pad();
        }
    }

    /// Whether Tehuti holds `button` from the virtual pad; reads only, so a new mirror can ask.
    #[must_use]
    pub fn virtual_pad_consumed(&self, button: tehuti_protocol::ButtonName) -> bool {
        self.virtual_pad_talk.contains(&button)
            || self
                .plugins
                .as_ref()
                .is_some_and(|plugins| plugins.pad_consumed(button))
    }

    /// Applies push-to-talk and plugin overrides; true keeps the button from the virtual pad.
    pub fn virtual_pad_button(
        &mut self,
        button: tehuti_protocol::ButtonName,
        state: tehuti_protocol::ButtonState,
    ) -> bool {
        let push_to_talk = self.dispatch.core().passthrough_push_to_talk();
        let mut talk = self.virtual_pad_talk.contains(&button);
        match state {
            tehuti_protocol::ButtonState::Pressed => {
                if !self.virtual_pad_buttons.contains(&button) {
                    self.virtual_pad_buttons.push(button);
                }
                if !push_to_talk.is_empty()
                    && push_to_talk.contains(&button)
                    && push_to_talk
                        .iter()
                        .all(|member| self.virtual_pad_buttons.contains(member))
                {
                    for member in push_to_talk {
                        if !self.virtual_pad_talk.contains(member) {
                            self.virtual_pad_talk.push(*member);
                        }
                    }
                    talk = true;
                }
            }
            tehuti_protocol::ButtonState::Released => {
                self.virtual_pad_buttons.retain(|held| *held != button);
                self.virtual_pad_talk.retain(|held| *held != button);
            }
        }
        let plugin = self
            .plugins
            .as_mut()
            .is_some_and(|plugins| plugins.pad_button(button, state, push_to_talk));
        talk || plugin
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
        self.release_due_keys();
        self.keep_invitation();
        self.poll_recording();
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

    fn physical_pointer_enabled(&self) -> bool {
        self.speech_context
            .as_deref()
            .and_then(|app_id| tehuti_core::own_context(self.contexts(), app_id, &self.speech_title))
            .is_some_and(tehuti_protocol::Context::physical_pointer)
    }

    pub fn host_pointer_motion(&mut self, delta_x: f64, delta_y: f64) {
        if self.physical_pointer_enabled() && delta_x.is_finite() && delta_y.is_finite() {
            self.pointer_motion(delta_x, delta_y);
        }
    }

    pub fn host_pointer_scroll(&mut self, horizontal: f64, vertical: f64) {
        if self.physical_pointer_enabled() && horizontal.is_finite() && vertical.is_finite() {
            self.pointer_scroll(horizontal, vertical);
        }
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "absolute device coordinates map to the output"
    )]
    pub fn host_pointer_absolute(&mut self, across: f64, down: f64) {
        if !self.physical_pointer_enabled() || !across.is_finite() || !down.is_finite() {
            return;
        }
        if let Some(output) = &self.output
            && let Some(geometry) = self.space.output_geometry(output)
        {
            let move_x = across
                .clamp(0.0, 1.0)
                .mul_add(f64::from(geometry.size.w), f64::from(geometry.loc.x))
                - self.pointer_location.x;
            let move_y = down
                .clamp(0.0, 1.0)
                .mul_add(f64::from(geometry.size.h), f64::from(geometry.loc.y))
                - self.pointer_location.y;
            self.host_pointer_motion(move_x, move_y);
        }
    }

    pub fn host_pointer_button(&mut self, button: tehuti_core::dispatch::PointerButton, pressed: bool) {
        if self.physical_pointer_enabled() {
            self.pointer_click(button, pressed);
        }
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "Smithay pointer coordinates and axis values use floating point"
    )]
    fn pointer_motion(&mut self, x: f64, y: f64) {
        tracing::trace!(x, y, "mouse dispatch motion");
        self.pointer_location.x += x;
        self.pointer_location.y += y;
        self.move_pointer();
    }
    #[expect(
        clippy::float_arithmetic,
        reason = "Smithay pointer coordinates and axis values use floating point"
    )]
    fn pointer_scroll(&mut self, x: f64, y: f64) {
        use smithay::backend::input::{Axis, AxisSource};
        let at_pointer = self.mouse_mode || self.physical_pointer_enabled();
        if at_pointer {
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
            .value(Axis::Horizontal, x)
            .value(Axis::Vertical, y);
        self.pointer.clone().axis(self, frame);
        self.pointer.clone().frame(self);
        if !at_pointer {
            self.move_pointer();
        }
    }
    fn pointer_click(&mut self, button: tehuti_core::dispatch::PointerButton, pressed: bool) {
        self.record_event(
            "pointer_click",
            &serde_json::json!({"button": format!("{button:?}"), "pressed": pressed}),
        );
        self.move_pointer();
        if pressed && let Some((window, _)) = self.space.element_under(self.pointer_location) {
            let key = window.toplevel().map(|toplevel| toplevel.wl_surface().clone());
            if let Some(key) = key
                && self.placement.window(&key).is_some()
            {
                if let Err(error) = self.placement.focus(&key) {
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
        self.dispatch.core().passthrough() && !self.dispatch.overlay_owns_pad()
    }

    pub fn set_volume_backend(&mut self, backend: crate::volume::Backend) {
        self.volume = Some(backend);
    }

    #[must_use]
    pub const fn adjuster(&self) -> Option<&tehuti_core::adjuster::Gauge> {
        self.dispatch.adjuster()
    }

    pub fn sample_sticks(&mut self, sticks: tehuti_core::dispatch::Sticks) {
        let _rejection = self.dispatch(DispatchInput::Sticks(sticks));
    }

    #[must_use]
    pub fn cursor_location(&self) -> Option<(f64, f64)> {
        (self.mouse_mode || self.physical_pointer_enabled())
            .then_some((self.pointer_location.x, self.pointer_location.y))
    }

    /// Where to draw the cursor on `output`, in its physical pixels.
    #[must_use]
    pub fn cursor_on(
        &self,
        output: &smithay::output::Output,
    ) -> Option<smithay::utils::Point<f64, smithay::utils::Physical>> {
        let scale = output.current_scale().fractional_scale();
        (self.mouse_mode || self.physical_pointer_enabled()).then(|| self.pointer_location.to_physical(scale))
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
        if matches!(
            event,
            Event::Button {
                name: tehuti_protocol::ButtonName::East,
                state: tehuti_protocol::ButtonState::Pressed,
                ..
            }
        ) && self.recording_failure.take().is_some()
        {
            return;
        }
        if matches!(event, Event::Speech { .. }) {
            let registrations = self
                .plugins
                .as_ref()
                .map_or_else(Vec::new, crate::plugins::Plugins::adjusters);
            let _rejection = self.dispatch(DispatchInput::Adjusters(registrations));
        }
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
        self.record_screen();
    }

    fn after_event(&mut self, event: &Event) {
        if let Event::Focus { app_id, title } = event {
            if self.speech_context != *app_id {
                tracing::info!(?app_id, "focused application changed");
            }
            self.speech_context.clone_from(app_id);
            self.speech_title.clone_from(title);
            self.refresh_speech_vocabulary();
        }
        self.keep_invitation();
    }

    /// Shows the session-start invitation whenever nothing is open; the session enables it.
    pub const fn enable_invitation(&mut self) {
        self.launcher.enable_invitation();
    }

    /// With nothing open, the invitation is the first thing on screen; a window closes it.
    fn keep_invitation(&mut self) {
        match self.launcher.invitation(
            !self.placement.is_empty() || self.speech_context.is_some(),
            self.choice_list().is_some()
                || self.dispatch.keyboard().is_open()
                || self.dispatch.overlay_owns_pad(),
            Instant::now(),
        ) {
            crate::launcher::InvitationView::Hidden => return,
            crate::launcher::InvitationView::Close => {
                let _rejection = self.dispatch(DispatchInput::CloseInvitation);
                return;
            }
            crate::launcher::InvitationView::Show => {}
        }
        let button = (!self.controller_names.is_empty()).then(|| match self.pad_family() {
            tehuti_core::pad::PadFamily::Letters => "Share",
            tehuti_core::pad::PadFamily::Symbols => "Create",
            tehuti_core::pad::PadFamily::Nintendo => "Capture",
        });
        let invitation = tehuti_core::launcher::Launcher::invitation(button);
        let _rejection = self.dispatch(DispatchInput::Launcher(Box::new(invitation)));
    }

    /// Feeds one input and every reply it asks for until its frame finishes.
    pub(crate) fn dispatch(&mut self, input: DispatchInput) -> Option<tehuti_core::plugin::Rejection> {
        // Loop ticks are not timeline events; sticks are sampled once per video frame.
        if let DispatchInput::Sticks(sticks) = &input {
            self.record_sample("input", "sticks", sticks);
        } else if !matches!(input, DispatchInput::Tick { .. }) {
            self.record_debug("input", &input);
        }
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
        if previous != self.dispatch.core().level() {
            self.dim_elapsed = core::time::Duration::ZERO;
        }
        self.update_window_dimming();
        self.reserve_keyboard_height();
        self.record_screen();
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
                let focused = window
                    .toplevel()
                    .is_some_and(|toplevel| self.placement.focused() == Some(toplevel.wl_surface()));
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

    pub(crate) fn launcher_icons(&self) -> Vec<String> {
        let Some(launcher) = self.dispatch.launcher() else {
            return Vec::new();
        };
        (0..launcher.list().slots().candidates.len())
            .map(|index| {
                launcher
                    .candidate_destination(index)
                    .and_then(|index| self.launcher.route(index))
                    .and_then(|route| route.steps.first())
                    .map_or_default(|step| step.icon.clone())
            })
            .collect()
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
        let prompt = crate::speech_vocabulary::prompt(
            self.contexts(),
            self.speech_context.as_deref(),
            &self.speech_title,
        );
        self.speech_process.as_mut().is_none_or(|process| {
            process
                .send_with_prompt(command, &prompt)
                .inspect_err(|error| tracing::warn!(%error, "speech command failed"))
                .is_ok()
        })
    }

    #[must_use]
    pub fn settings_buffer(
        &self,
        size: (i32, i32),
    ) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> {
        self.settings_display.artwork.borrow_mut().buffer(
            self.settings(),
            &crate::speech::speech_label(self.speech_process.as_ref()),
            size,
        )
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
        let bindings = store.controller_bindings().unwrap_or_else(|error| {
            tracing::warn!(%error, "controller bindings unreadable; using context defaults");
            std::collections::BTreeMap::new()
        });
        let _bindings_rejection = self.dispatch(DispatchInput::ControllerBindings(bindings));
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

    pub fn activate_controller(&mut self, name: String) {
        let _rejection = self.dispatch(DispatchInput::ActiveController(name));
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
        if matches!(effect, DispatchEffect::MovePointer { .. }) {
            self.record_sample("effect", "pointer", &effect);
        } else if matches!(effect, DispatchEffect::Scroll { .. }) {
            self.record_sample("effect", "scroll", &effect);
        } else if !matches!(
            effect,
            DispatchEffect::PollSpeech(_)
                | DispatchEffect::PollSources
                | DispatchEffect::Supervise
                // Loop ticks offered to plugins are not timeline events either.
                | DispatchEffect::QueryPlugin(Event::Tick { .. })
                | DispatchEffect::DispatchPlugin {
                    event: Event::Tick { .. },
                    ..
                }
        ) {
            self.record_debug("effect", &effect);
        }
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
            | DispatchEffect::LauncherUsed(_)
            | DispatchEffect::OpenLauncher(_)
            | DispatchEffect::ReadVolume
            | DispatchEffect::SetVolume(_)
            | DispatchEffect::AdjusterValue { .. }
            | DispatchEffect::SettingsMode(_)
            | DispatchEffect::PersistMode(_)
            | DispatchEffect::SettingsBarFont(_)
            | DispatchEffect::SettingsScale(_)
            | DispatchEffect::WallpaperPreview(_)
            | DispatchEffect::WallpaperSave(_)
            | DispatchEffect::SettingsController(_)
            | DispatchEffect::PersistControllerTalk(_, _)
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
            | DispatchEffect::FocusGapExpired
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
        if command == tehuti_core::menu::Command::ToggleRecording {
            if self.recording_active() {
                self.stop_recording();
            } else {
                self.start_recording();
            }
            return;
        }
        if command == tehuti_core::menu::Command::StartRecording {
            self.start_recording();
            return;
        }
        if command == tehuti_core::menu::Command::StopRecording {
            self.stop_recording();
            return;
        }
        if command == tehuti_core::menu::Command::Screenshot {
            match crate::screenshot::save(self) {
                Ok(path) => self.show_notice(&format!("SCREENSHOT SAVED: {}", path.display())),
                Err(error) => self.show_notice(&format!("SCREENSHOT FAILED: {error}")),
            }
            return;
        }
        if command == tehuti_core::menu::Command::Restart {
            self.apply(&Action::Restart);
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
            .and_then(|socket| {
                crate::system_menu::execute(&self.menu_config, command, socket, self.menu_failures.0.clone())
            });
        if let Err(error) = result {
            let label = self.menu_config.label(command);
            tracing::warn!(?command, %error, "system menu command failed");
            self.show_notice(&format!("{label} FAILED: {error}"));
        }
    }

    fn poll_menu_failures(&mut self) {
        while let Ok(failure) = self.menu_failures.1.try_recv() {
            tracing::warn!(%failure, "system menu command exited with an error");
            self.show_notice(&failure);
        }
    }

    fn open_launcher(&mut self, request: &tehuti_core::unit_verbs::LauncherRequest) {
        self.show_launcher(request, false);
    }

    fn show_launcher(&mut self, request: &tehuti_core::unit_verbs::LauncherRequest, typed: bool) {
        let launcher = self.launcher.show(
            request,
            typed,
            self.speech_context.as_deref(),
            &self.speech_title,
            self.dispatch.core().contexts(),
            self.dispatch.core().level(),
        );
        let _rejection = self.dispatch(DispatchInput::Launcher(Box::new(launcher)));
    }

    fn follow_launcher_route(&mut self, route: crate::destinations::Route) {
        use crate::destinations::Target;
        for step in route.steps {
            match step.target {
                Target::Workspace(identity) => {
                    if let Err(error) = self
                        .placement
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
                    if let Err(error) = self.placement.focus_destination(identity) {
                        tracing::warn!(%error, "launcher focus failed");
                    }
                    self.place_layout();
                }
                Target::Application(app_id) => {
                    if let Some(window) = self
                        .launcher
                        .windows()
                        .iter()
                        .find(|window| window.app_id == app_id)
                    {
                        let identity = window.identity;
                        if let Err(error) = self.placement.focus_destination(identity) {
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
        if !self.launcher.launch_url(url, self.launch_socket.as_deref()) {
            self.show_launch_failure();
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
            DispatchEffect::LauncherUsed(usage) => {
                tracing::info!("{}", crate::launcher_log::line(&usage));
            }
            DispatchEffect::ChooseLauncher(index) => {
                if let Some(route) = self
                    .launcher
                    .choose(index, self.placement.is_empty(), Instant::now())
                {
                    self.follow_launcher_route(route);
                }
            }
            DispatchEffect::ReadVolume => self.volume_request(crate::volume::Request::Read),
            DispatchEffect::SetVolume(value) => self.volume_request(crate::volume::Request::Set(value)),
            DispatchEffect::AdjusterValue {
                plugin,
                name,
                value,
                finished,
            } => {
                if let Some(plugins) = &mut self.plugins {
                    plugins.adjuster_value(plugin, name, value, finished);
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
            DispatchEffect::PersistControllerTalk(controller_name, button) => {
                let mut bindings = self.settings().input.controller_bindings.clone();
                if let Some(button) = button {
                    let _previous = bindings.insert(controller_name, button);
                } else {
                    let _previous = bindings.remove(&controller_name);
                }
                self.save_input_choice(|store| store.set_controller_bindings(&bindings));
                if let Some(name) = self.settings().input.active_controller.clone() {
                    self.activate_controller(name);
                }
            }
            DispatchEffect::PersistController(choice) => self.choose_controller(choice, true),
            DispatchEffect::SettingsMicrophone(choice) => self.choose_microphone(&choice),
            DispatchEffect::ListMicrophones => {
                self.microphone_listing = Some(crate::input_choice_store::list_microphones());
            }
            DispatchEffect::PasswordSpeechIgnored => {
                tracing::info!("push-to-talk ignored: password field active");
            }
            DispatchEffect::RunMenuCommand(command) => self.run_menu_command(command),
            DispatchEffect::MovePointer { x, y } => {
                self.record_event("pointer_motion", &serde_json::json!({"x": x, "y": y}));
                #[expect(
                    clippy::float_arithmetic,
                    reason = "core pointer deltas use millionths of a pixel"
                )]
                self.pointer_motion(f64::from(x) / 1_000_000.0, f64::from(y) / 1_000_000.0);
            }
            DispatchEffect::Scroll { x, y } => {
                self.record_event("pointer_scroll", &serde_json::json!({"x": x, "y": y}));
                #[expect(
                    clippy::float_arithmetic,
                    reason = "core scroll deltas use millionths of a unit"
                )]
                self.pointer_scroll(f64::from(x) / 1_000_000.0, f64::from(y) / 1_000_000.0);
            }
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
                self.poll_microphones();
                self.poll_menu_failures();
                self.poll_volume();
            }
            DispatchEffect::ConfirmPressed(pressed) => self.confirmation.confirm_pressed(pressed),
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
            DispatchEffect::FocusGapExpired => tracing::info!(
                windows = ?self.launcher.windows(),
                "focus stayed on no window past the grace; the desktop takes the pad"
            ),
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
        self.confirmation.focus_changed(self.live_secret_focus());
        self.sync_secret_prompt();
    }

    fn live_secret_focus(&self) -> Option<WlSurface> {
        self.keyboard
            .current_focus()
            .filter(smithay::reexports::wayland_server::Resource::is_alive)
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
        let requested = self
            .confirmation
            .request(reference, &target, Some(&focus), self.clock_elapsed());
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
        self.confirmation
            .text(self.live_secret_focus().as_ref(), self.clock_elapsed())
    }

    /// Records that a damaged frame containing the confirmation was submitted.
    pub fn note_secret_confirmation_frame_submitted(&mut self) {
        if self.secret_confirmation_text().is_some() {
            self.confirmation.frame_submitted();
        }
    }

    fn handle_secret_confirmation_button(&mut self, state: tehuti_protocol::ButtonState) -> bool {
        let consumed =
            self.confirmation
                .button(state, self.live_secret_focus().as_ref(), self.clock_elapsed());
        self.sync_secret_prompt();
        consumed
    }

    fn poll_secret_lookup(&mut self) {
        let focus = self.live_secret_focus();
        let Some(delivery) = self.confirmation.poll(focus.as_ref(), self.clock_elapsed()) else {
            return;
        };
        let Some(focus) = focus else {
            return;
        };
        let keymap = self.text_keys.clone();
        let seat = self.seat.clone();
        let delivered = delivery.deliver(&keymap, |(keycode, modifiers)| {
            focus.modifiers(&seat, self, modifiers, SERIAL_COUNTER.next_serial());
            self.tap_keycode(keycode);
            focus.modifiers(
                &seat,
                self,
                ModifiersState::default(),
                SERIAL_COUNTER.next_serial(),
            );
        });
        if !delivered {
            self.confirmation.delivery_unavailable(self.clock_elapsed());
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
                    self.confirmation.delivery_unavailable(self.clock_elapsed());
                }
            }
            Action::Quit | Action::Restart => {
                self.restart_requested = *action == Action::Restart;
                self.speech_input(tehuti_core::speech::SpeechInput::Cancel);
                self.quit_requested = true;
            }
            Action::Launch { desktop_entry } => self.launch_application(desktop_entry),
            Action::Close => {
                if let Some(key) = self.placement.focused().cloned() {
                    self.remove_layout_window(&key, true);
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

    pub(super) fn show_recording_failure(&mut self, error: &str) {
        self.recording_failure = Some(format!("RECORDING FAILED: {error}\nESC OR B TO DISMISS"));
    }

    fn show_notice(&mut self, text: &str) {
        let _effect = self.placement.window_action(tehuti_core::windows::Input::Notify);
        self.notice_text = Some(text.into());
        self.notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
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
        let names: Vec<&str> = [
            (Modifier::Ctrl, "Control_L"),
            (Modifier::Alt, "Alt_L"),
            (Modifier::Shift, "Shift_L"),
            (Modifier::Super, "Super_L"),
        ]
        .into_iter()
        .filter(|(modifier, _)| mods.contains(modifier))
        .map(|(_, name)| name)
        .collect();
        let Some(held) = self.resolve_modifiers(&names) else {
            return;
        };
        self.tap_held(keycode, &held);
    }

    fn resolve_modifiers(&mut self, names: &[&str]) -> Option<Vec<Keycode>> {
        let held = names
            .iter()
            .map(|name| self.resolve_keysym(name))
            .collect::<Option<Vec<_>>>();
        if held.is_none() {
            tracing::warn!(?names, "modifier unavailable in active keymap");
        }
        held
    }

    pub const fn instant_taps(&mut self) {
        self.taps.instant_taps();
    }

    fn apply_tap_events(&mut self, events: Vec<(Keycode, bool)>) {
        for (code, pressed) in events {
            self.send_keycode(
                code,
                if pressed {
                    smithay::backend::input::KeyState::Pressed
                } else {
                    smithay::backend::input::KeyState::Released
                },
            );
        }
    }

    fn tap_held(&mut self, keycode: Keycode, modifiers: &[Keycode]) {
        let events = self.taps.tap(self.clock_elapsed(), keycode, modifiers);
        self.apply_tap_events(events);
    }

    pub fn release_due_keys(&mut self) {
        let events = self.taps.tick(self.clock_elapsed());
        self.apply_tap_events(events);
    }

    pub fn release_all_held(&mut self) {
        let events = self.taps.release_all();
        self.apply_tap_events(events);
    }

    pub fn release_timer_failed(&mut self) {
        let events = self.taps.timer_failed();
        self.apply_tap_events(events);
    }

    pub fn take_release_request(&mut self) -> Option<core::time::Duration> {
        self.taps.take_release_request(self.clock_elapsed())
    }

    pub fn release_timer_fired(&mut self) -> Option<core::time::Duration> {
        let (events, delay) = self.taps.timer_fired(self.clock_elapsed());
        self.apply_tap_events(events);
        delay
    }

    /// Presses the named modifier keys around one key tap.
    fn tap_keycode_with_keysyms(&mut self, keycode: Keycode, names: &[&str]) {
        let held = names
            .iter()
            .map(|name| self.resolve_keysym(name))
            .collect::<Option<Vec<_>>>();
        let Some(held) = held else {
            tracing::warn!(?names, "modifier unavailable in active keymap");
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
    ///
    /// Meta is the Guide button and, while Meta is held, the arrows are the d-pad: both go to the core as pad buttons and
    /// the client never sees them.
    pub fn host_key(&mut self, keycode: Keycode, pressed: bool) {
        if pressed && keycode.raw() == 9 && self.recording_failure.take().is_some() {
            return;
        }
        self.record_event(
            "key",
            &serde_json::json!({"keycode": keycode.raw(), "pressed": pressed}),
        );
        if self.host_key_as_pad(keycode, pressed) {
            return;
        }
        if self.host_key_in_launcher(keycode, pressed) {
            return;
        }
        let state = if pressed {
            smithay::backend::input::KeyState::Pressed
        } else {
            smithay::backend::input::KeyState::Released
        };
        self.send_keycode(keycode, state);
    }

    /// While the launcher is open, typing filters it, Enter opens the selection and Escape closes it.
    ///
    /// This is the fallback when speech is down; the client never sees these keys.
    fn host_key_in_launcher(&mut self, keycode: Keycode, pressed: bool) -> bool {
        use tehuti_protocol::{ButtonName, ButtonState};
        const KEY_ESC: u32 = 1;
        const KEY_BACKSPACE: u32 = 14;
        const KEY_ENTER: u32 = 28;
        const KEY_KPENTER: u32 = 96;
        if !pressed {
            let consumed = self.consumed_host_keys.contains(&keycode);
            self.consumed_host_keys.retain(|held| *held != keycode);
            return consumed;
        }
        let Some(launcher) = self
            .dispatch
            .launcher()
            .filter(|launcher| launcher.list().is_open())
        else {
            return false;
        };
        let query = launcher.list().slots().heard.clone();
        let family = self.pad_family();
        let code = keycode.raw().saturating_sub(XKB_KEYCODE_OFFSET);
        let press = |this: &mut Self, name: ButtonName| {
            for state in [ButtonState::Pressed, ButtonState::Released] {
                this.handle(&Event::Button { name, state });
            }
        };
        match code {
            KEY_ENTER | KEY_KPENTER => press(self, family.button(tehuti_core::pad::Role::Confirm)),
            KEY_ESC => press(self, family.button(tehuti_core::pad::Role::Back)),
            KEY_BACKSPACE => {
                let mut query = query;
                let _removed = query.pop();
                self.retype_launcher(query);
            }
            _ => {
                let keyboard = self.keyboard.clone();
                let text = keyboard.with_xkb_state(self, |context| {
                    let xkb = context
                        .xkb()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let layout = xkb.active_layout();
                    xkb.raw_syms_for_key_in_layout(keycode, layout)
                        .first()
                        .and_then(|symbol| char::from_u32(xkb::keysym_to_utf32(*symbol)))
                        .filter(|character| *character != '\0')
                        .map_or_default(String::from)
                });
                let held = self.keyboard.modifier_state();
                if held.ctrl || held.alt || held.logo || text.is_empty() || text.chars().any(char::is_control)
                {
                    return false;
                }
                self.retype_launcher(format!("{query}{text}"));
            }
        }
        self.consumed_host_keys.push(keycode);
        true
    }

    fn retype_launcher(&mut self, query: String) {
        let request = tehuti_core::unit_verbs::LauncherRequest {
            kind: tehuti_core::unit_verbs::DestinationKind::Application,
            query,
        };
        self.show_launcher(&request, true);
    }

    fn host_key_as_pad(&mut self, keycode: Keycode, pressed: bool) -> bool {
        use tehuti_protocol::{ButtonName, ButtonState};
        const KEY_UP: u32 = 103;
        const KEY_LEFT: u32 = 105;
        const KEY_RIGHT: u32 = 106;
        const KEY_DOWN: u32 = 108;
        const KEY_LEFTMETA: u32 = 125;
        const KEY_RIGHTMETA: u32 = 126;
        let button_state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        let code = keycode.raw().saturating_sub(XKB_KEYCODE_OFFSET);
        if matches!(code, KEY_LEFTMETA | KEY_RIGHTMETA) {
            let was_held = !self.meta_held.is_empty();
            self.meta_held.retain(|held| *held != code);
            if pressed {
                self.meta_held.push(code);
            }
            if was_held == self.meta_held.is_empty() {
                self.handle(&Event::Button {
                    name: ButtonName::Guide,
                    state: button_state,
                });
            }
            return true;
        }
        let name = match code {
            KEY_UP => ButtonName::DpadUp,
            KEY_DOWN => ButtonName::DpadDown,
            KEY_LEFT => ButtonName::DpadLeft,
            KEY_RIGHT => ButtonName::DpadRight,
            _ => return false,
        };
        if pressed {
            let reached_client = !self.consumed_host_keys.contains(&keycode)
                && self.keyboard.pressed_keys().contains(&keycode);
            if self.meta_held.is_empty() || reached_client {
                return false;
            }
            if !self.consumed_host_keys.contains(&keycode) {
                self.consumed_host_keys.push(keycode);
            }
        } else if self.consumed_host_keys.contains(&keycode) {
            self.consumed_host_keys.retain(|held| *held != keycode);
        } else {
            return false;
        }
        self.handle(&Event::Button {
            name,
            state: button_state,
        });
        true
    }

    /// Switches to the workspace holding `window`, so a moved column keeps the focus.
    fn follow_window(&mut self, window: Option<WlSurface>) {
        if let Some(window) = window
            && self.placement.follow(&window)
        {
            self.place_layout();
        }
    }

    /// Releases every held host and pad key, so a key held when the host window loses focus or the seat pauses does not stick.
    pub fn release_host_keys(&mut self) {
        self.held_pad_keys.clear();
        self.meta_held.clear();
        self.consumed_host_keys.clear();
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
        self.flush_focus_change();
    }

    /// Runs the dispatch a keyboard focus change deferred, now that smithay's keyboard lock is free.
    fn flush_focus_change(&mut self) {
        if let Some(input) = self.focus_change_dispatch.take() {
            let events = self.taps.flush_focus();
            self.apply_tap_events(events);
            let _rejection = self.dispatch(input);
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
        for character in text.chars() {
            if character == '\n' {
                self.tap_keycode(Keycode::new(
                    KeyName::Enter.evdev_code().saturating_add(XKB_KEYCODE_OFFSET),
                ));
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
            self.tap_keycode_with_keysyms(keycode, &modifier_keys(modifiers));
        }
    }

    fn millis_since_start(&self) -> u32 {
        u32::try_from(self.clock_elapsed().as_millis()).unwrap_or(u32::MAX)
    }

    fn apply_layout(&mut self, action: tehuti_protocol::LayoutAction) {
        self.apply_layout_command(tehuti_core::layout::command_for(action));
    }

    /// Applies a tiling command and places the mapped windows.
    ///
    /// Moving a column to another workspace takes the focus along with it.
    pub fn apply_layout_command(&mut self, command: tehuti_core::layout::Command) {
        use tehuti_core::layout::Command;
        let moved = self.placement.focused().cloned();
        self.placement.apply(command);
        self.place_layout();
        if matches!(
            command,
            Command::MoveColumnToWorkspaceUp | Command::MoveColumnToWorkspaceDown
        ) {
            self.follow_window(moved);
        }
    }

    /// The next opening slot while the launcher is visible.
    #[must_use]
    pub fn launcher_slot(&self) -> Option<tehuti_core::layout::Rect> {
        let _launcher = self.dispatch.launcher()?;
        self.placement.preview()
    }

    /// The tiling layout every mapped window is placed by.
    #[must_use]
    pub fn layout_snapshot(&self) -> tehuti_core::layout::Snapshot {
        self.placement.snapshot()
    }

    #[must_use]
    pub fn focus_outline(
        &self,
        output: &Output,
    ) -> Vec<smithay::backend::renderer::element::solid::SolidColorRenderElement> {
        use smithay::utils::Rectangle;
        let placed = self.placement.placed(|_| 0.0);
        let Some(focused) = placed.iter().find(|placed| placed.focused) else {
            return Vec::new();
        };
        if focused
            .tile
            .as_ref()
            .is_some_and(|(tile, _)| tile.mode == tehuti_core::layout::TileMode::Fullscreen)
        {
            return Vec::new();
        }
        let window = focused.window;
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
        let snapshot = self.placement.snapshot();
        let windows = self
            .placement
            .active_tiles()
            .into_iter()
            .filter_map(|(window, focused)| {
                let toplevel = window.toplevel()?;
                let title = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
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
                })?;
                Some((title, focused))
            })
            .collect();
        let mut bar = self.dispatch.core().bar().clone().with_windows(windows);
        if let Some(identity) = self.placement.focused_destination()
            && let Some(name) = self
                .launcher
                .table()
                .name_for(&crate::destinations::Target::Window(identity))
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
                .launcher
                .table()
                .name_for(&crate::destinations::Target::Workspace(workspace.workspace.0))
        {
            bar = bar.with_destination(name.into());
        }
        if let Some(status) = self.plugins.as_ref().and_then(crate::plugins::Plugins::status) {
            bar = bar.with_status(status);
        }
        if let Some(text) = self
            .workspace_feedback
            .text(self.clock_elapsed(), self.launcher.table())
        {
            bar = bar.with_navigation(text);
        }
        self.recording_bar(bar)
    }

    fn recording_bar(&self, bar: tehuti_core::bar::Bar) -> tehuti_core::bar::Bar {
        let mut bar = bar.with_recording(self.recording_active());
        bar.font = self.settings().bar_font;
        bar
    }

    /// Updates the layout to the output's logical size and configures its windows.
    #[expect(
        clippy::float_arithmetic,
        reason = "reserve the bar in logical output coordinates"
    )]
    pub fn fill_output(&mut self) {
        use tehuti_core::layout::{OutputGeometry, Rect};
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
        let result = self.placement.resize(geometry);
        if let Err(error) = result {
            tracing::warn!(%error, "output layout failed");
        }
        self.place_layout();
    }

    /// Writes the layout before and after a change to the session log.
    fn log_layout(&mut self) {
        let change = self
            .placement
            .observe(|window| window.toplevel().and_then(toplevel_app_id));
        if let Some(change) = change {
            tracing::info!(
                before = change.before.as_deref().unwrap_or("none"),
                after = %change.after,
                "layout changed"
            );
        }
    }

    fn log_window_size(
        &self,
        window: &LayoutWindow,
        old: smithay::utils::Size<i32, smithay::utils::Logical>,
        new: smithay::utils::Size<i32, smithay::utils::Logical>,
        source: &str,
    ) {
        if old == new {
            return;
        }
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let (app_id, title) = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
            let attributes = states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()?;
            let attributes = attributes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Some((
                attributes.app_id.clone()?,
                attributes.title.clone().unwrap_or_default(),
            ))
        })
        .unwrap_or_default();
        if tehuti_core::own_context(self.contexts(), &app_id, &title)
            .is_some_and(tehuti_protocol::Context::log_window_size)
        {
            tracing::info!(
                app_id = %app_id,
                old_width = old.w,
                old_height = old.h,
                new_width = new.w,
                new_height = new.h,
                source,
                "window size changed"
            );
        }
    }

    /// Places every window and sends each the configure its tile asks for.
    fn place_layout(&mut self) {
        self.place_windows(true);
    }

    /// Places every window; with `configure` unset, only positions change.
    ///
    /// A commit only repositions: a client that picks its own size must not be
    /// answered with a fresh configure on every frame.
    fn place_windows(&mut self, configure: bool) {
        let snapshot = self.placement.snapshot();
        self.workspace_feedback.update(&snapshot, self.clock_elapsed());
        self.log_layout();
        for placed in self
            .placement
            .placed(|window| f64::from(window.client_geometry().size.w))
        {
            let window = placed.window;
            if let Some(position) = placed.helper_position {
                Self::place_helper(
                    &mut self.space,
                    self.output.as_ref(),
                    window,
                    position,
                    placed.focused,
                    configure,
                );
                continue;
            }
            let Some((tile, border)) = placed.tile.as_ref() else {
                self.space.unmap_elem(window);
                continue;
            };
            let location = self.size_layout_window(window, tile, *border, configure);
            self.space.map_element(window.clone(), location, placed.focused);
            // Space only reports output entry once a buffer gives the window a
            // view. Clients such as mpv size their first buffer from the output
            // scale they learn on entry, so announce it before the first
            // configure instead of after the first (wrongly scaled) frame.
            if let (Some(output), Some(toplevel)) = (&self.output, window.toplevel()) {
                output.enter(toplevel.wl_surface());
                Self::announce_scale(output, toplevel.wl_surface());
            }
            if configure && let Some(toplevel) = window.toplevel() {
                Self::configure_layout_window(toplevel, tile, placed.focused);
            }
        }
        self.sync_window_focus();
        self.update_window_dimming();
    }

    fn size_layout_window(
        &self,
        window: &LayoutWindow,
        tile: &tehuti_core::layout::TileView,
        border: f64,
        configure: bool,
    ) -> smithay::utils::Point<i32, smithay::utils::Logical> {
        let old_size = window.layout_size();
        window.set_size(
            smithay::utils::Size::<f64, smithay::utils::Logical>::from((
                tile.requested_client.width,
                tile.requested_client.height,
            ))
            .to_i32_round(),
        );
        self.log_window_size(
            window,
            old_size,
            window.layout_size(),
            if configure {
                "compositor configure"
            } else {
                "commit placement"
            },
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
        smithay::utils::Point::<f64, smithay::utils::Logical>::from((placement.x, placement.y)).to_i32_round()
    }

    fn place_helper(
        space: &mut smithay::desktop::Space<LayoutWindow>,
        output: Option<&Output>,
        window: &LayoutWindow,
        position: (f64, f64),
        focused: bool,
        configure: bool,
    ) {
        space.map_element(
            window.clone(),
            smithay::utils::Point::<f64, smithay::utils::Logical>::from(position).to_i32_round(),
            focused,
        );
        if let Some(toplevel) = window.toplevel() {
            if let Some(output) = output {
                output.enter(toplevel.wl_surface());
                Self::announce_scale(output, toplevel.wl_surface());
            }
            toplevel.with_pending_state(|pending| {
                pending.size = None;
                pending.states = smithay::wayland::shell::xdg::ToplevelStateSet::default();
                if focused {
                    let _changed = pending.states.set(xdg_toplevel::State::Activated);
                }
            });
            if configure && toplevel.is_initial_configure_sent() {
                let _serial = toplevel.send_pending_configure();
            }
        }
    }

    fn configure_layout_window(
        toplevel: &ToplevelSurface,
        tile: &tehuti_core::layout::TileView,
        focused: bool,
    ) {
        Self::set_layout_state(toplevel, tile, focused);
        if toplevel.is_initial_configure_sent() {
            let _serial = toplevel.send_pending_configure();
        }
    }

    /// Writes a tile's size and states into the pending configure.
    ///
    /// Every tile that is not fullscreen is tiled on all four edges, so clients
    /// take the size they are given instead of choosing their own.
    fn set_layout_state(toplevel: &ToplevelSurface, tile: &tehuti_core::layout::TileView, focused: bool) {
        let tiled = !tile.fullscreen_requested;
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
                (xdg_toplevel::State::TiledLeft, tiled),
                (xdg_toplevel::State::TiledRight, tiled),
                (xdg_toplevel::State::TiledTop, tiled),
                (xdg_toplevel::State::TiledBottom, tiled),
                (xdg_toplevel::State::Maximized, false),
            ] {
                if enabled {
                    let _changed = pending.states.set(state);
                } else {
                    let _changed = pending.states.unset(state);
                }
            }
        });
    }

    /// Answers a client's maximize or fullscreen request with exactly one configure.
    ///
    /// Fullscreen requests go through the layout; maximize requests keep the
    /// tile and only hear its current size and states.
    fn answer_state_request(&mut self, surface: &ToplevelSurface, fullscreen: Option<bool>) {
        let mapped = self.placement.window(surface.wl_surface()).is_some();
        if let Some(fullscreen) = fullscreen
            && let Err(error) = self.placement.fullscreen(surface.wl_surface(), fullscreen)
        {
            tracing::warn!(%error, "fullscreen request failed");
        }
        if !mapped {
            return;
        }
        if let Some(placed) = self
            .placement
            .placed(|_| 0.0)
            .iter()
            .find(|placed| placed.key == surface.wl_surface())
            && let Some((tile, _)) = &placed.tile
        {
            Self::set_layout_state(surface, tile, placed.focused);
        }
        if surface.has_pending_changes() || Self::awaits_answer(surface) {
            let _serial = surface.send_configure();
        }
        self.place_layout();
    }

    /// A request that changes nothing still deserves one configure, unless the
    /// client has yet to acknowledge one: a client that re-asks after every
    /// configure would otherwise loop forever.
    fn awaits_answer(surface: &ToplevelSurface) -> bool {
        surface.is_initial_configure_sent()
            && smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
                states
                    .data_map
                    .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                    .is_some_and(|data| {
                        data.lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pending_configures()
                            .is_empty()
                    })
            })
    }

    fn remove_layout_window(&mut self, key: &WlSurface, close: bool) {
        if let Some(window) = self.placement.unmap(key) {
            if close && let Some(toplevel) = window.toplevel() {
                toplevel.send_close();
            }
            self.space.unmap_elem(&window);
            self.place_layout();
        }
    }

    fn launch_application(&mut self, desktop_entry: &tehuti_protocol::Text) {
        let installed = self.launcher.installed();
        let decision = self.placement.window_action(tehuti_core::windows::Input::Launch {
            desktop_entry: desktop_entry.as_str().into(),
            installed,
        });
        if decision != Some(tehuti_core::windows::Effect::Launch) {
            self.show_launch_failure();
            return;
        }
        if !self
            .launcher
            .launch_application(desktop_entry.as_str(), self.launch_socket.as_deref())
        {
            self.show_launch_failure();
        }
    }

    pub fn set_launch_socket(&mut self, socket: std::ffi::OsString) {
        self.launch_socket = Some(socket);
    }

    fn show_launch_failure(&mut self) {
        self.launcher.launch_failed();
        self.notice_text = Some("APPLICATION UNAVAILABLE".into());
        self.notice_until = self.clock_now().checked_add(core::time::Duration::from_secs(3));
    }

    fn poll_launchers(&mut self) {
        let now = Instant::now();
        let failures = self.launcher.poll(now);
        let summary = crate::launcher::Failure::summary(&failures);
        if failures.iter().any(|failure| !failure.early) {
            self.show_launch_failure();
        }
        if let Some(text) = summary {
            self.launcher.launch_failed();
            self.show_notice(&text);
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
        match crate::destinations::save(&self.launcher.paths().config, name, &window) {
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
        let snapshot = self.placement.snapshot();
        self.launcher.update_workspaces(&snapshot);
        let windows: Vec<_> = self
            .placement
            .placed(|_| 0.0)
            .iter()
            .filter_map(|placed| {
                if placed.helper {
                    return None;
                }
                let window = placed.window;
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
                        identity: placed.identity,
                        app_id: attributes.app_id.clone().unwrap_or_default(),
                        title: attributes.title.clone().unwrap_or_default(),
                        chip,
                    })
                })
            })
            .collect();
        if self.launcher.update_windows(windows) {
            self.refresh_speech_vocabulary();
        }
    }

    fn sync_window_focus(&mut self) {
        self.sync_destination_windows();
        let focus = self.placement.focused().cloned();
        self.keyboard
            .clone()
            .set_focus(self, focus.clone(), SERIAL_COUNTER.next_serial());
        self.flush_focus_change();
        if let Some(window) = focus
            .as_ref()
            .and_then(|surface| self.window_for_root(surface))
            .cloned()
        {
            if let Some(toplevel) = window.toplevel() {
                self.update_focus(toplevel);
            }
        } else if let Some((surface, app_id)) = self.window_focus.take() {
            tracing::debug!(
                ?app_id,
                windows = ?self.launcher.windows(),
                "focus left every window"
            );
            if !self.pad_passthrough() {
                self.dispatch.core_mut().set_context_ancestors(None, Vec::new());
            }
            self.unfocused_window = Some(surface);
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
                self.unfocused_window = None;
                let windows = self
                    .placement
                    .windows()
                    .filter_map(|window| window.toplevel().map(|toplevel| toplevel.wl_surface().clone()))
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
        self.placement.window(root)
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        self.launcher.shutdown();
    }
}

/// The real modifier keys a typed character needs; a mask alone does not reach every client.
fn modifier_keys(modifiers: ModifiersState) -> Vec<&'static str> {
    [
        (modifiers.ctrl, "Control_L"),
        (modifiers.alt, "Alt_L"),
        (modifiers.shift, "Shift_L"),
        (modifiers.logo, "Super_L"),
        (modifiers.iso_level3_shift, "ISO_Level3_Shift"),
    ]
    .into_iter()
    .filter_map(|(held, name)| held.then_some(name))
    .collect()
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
            if let Some(requested) = requested_size(&root) {
                let old = window.replace_requested_geometry(requested);
                self.log_window_size(&window, old, requested, "client request");
            }
            window.on_commit();
            self.log_window_size(
                &window,
                window.replace_committed_size(window.client_geometry().size),
                window.client_geometry().size,
                "client commit",
            );
            let membership_changed = window
                .toplevel()
                .is_some_and(|toplevel| self.update_helper(toplevel));
            if membership_changed {
                self.place_layout();
            } else if previous != Some((window.client_geometry(), window.mapped())) {
                self.place_windows(false);
            }
            if let Some(toplevel) = window.toplevel()
                && !toplevel.is_initial_configure_sent()
            {
                let _serial = toplevel.send_configure();
            }
        }
        self.record_surface_change();
    }
}

fn requested_size(root: &WlSurface) -> Option<smithay::utils::Size<i32, smithay::utils::Logical>> {
    smithay::wayland::compositor::with_states(root, |states| {
        states
            .cached_state
            .get::<smithay::wayland::shell::xdg::SurfaceCachedState>()
            .current()
            .geometry
            .map(|geometry| geometry.size)
    })
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
    fn is_helper(surface: &ToplevelSurface) -> bool {
        let untitled = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .is_none_or(|data| {
                    data.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .title
                        .as_deref()
                        .is_none_or(str::is_empty)
                })
        });
        untitled
            && smithay::backend::renderer::utils::with_renderer_surface_state(surface.wl_surface(), |state| {
                state.buffer().is_none_or(|buffer| {
                    smithay::backend::renderer::buffer_dimensions(buffer)
                        .is_some_and(|size| size.w <= 64 && size.h <= 64)
                })
            })
            .unwrap_or(true)
    }

    fn update_helper(&mut self, surface: &ToplevelSurface) -> bool {
        match self
            .placement
            .set_helper(surface.wl_surface(), Self::is_helper(surface))
        {
            Ok(changed) => changed,
            Err(error) => {
                tracing::warn!(%error, "helper membership failed");
                false
            }
        }
    }

    fn map_toplevel(&mut self, surface: ToplevelSurface) {
        self.fill_output();
        let app_id = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|attributes| attributes.lock().ok()?.app_id.clone())
        });
        let windows = self
            .placement
            .windows()
            .filter_map(|window| window.toplevel().map(|toplevel| toplevel.wl_surface().clone()))
            .collect::<Vec<_>>();
        let ancestors = crate::window_context::ancestors(surface.wl_surface(), &windows, &self.display);
        let title = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|attributes| attributes.lock().ok()?.title.clone())
        })
        .unwrap_or_default();
        let context = app_id
            .as_deref()
            .and_then(|app_id| tehuti_core::own_context(self.contexts(), app_id, &title))
            .or_else(|| {
                ancestors.iter().find_map(|ancestor| {
                    self.contexts().iter().find(|context| {
                        ancestor != "*" && context.app_id() == ancestor && context.title().is_none()
                    })
                })
            });
        let rules = Self::window_rules(&surface, context);
        let key = surface.wl_surface().clone();
        let window = LayoutWindow::new(Window::new_wayland_window(surface));
        match self.placement.map(key, window, rules) {
            Ok(()) => self.place_layout(),
            Err(error) => tracing::warn!(%error, "window layout failed"),
        }
    }

    fn window_rules(
        surface: &ToplevelSurface,
        context: Option<&tehuti_protocol::Context>,
    ) -> tehuti_core::layout::WindowRules {
        tehuti_core::layout::WindowRules {
            kind: if Self::is_helper(surface) {
                tehuti_core::layout::WindowKind::Helper
            } else {
                tehuti_core::layout::WindowKind::Ordinary
            },
            open_fullscreen: context.is_some_and(tehuti_protocol::Context::open_fullscreen),
            default_column_width: context
                .and_then(tehuti_protocol::Context::window_column_width)
                .map(|width| Some(tehuti_core::layout::PresetSize::Proportion(width))),
            default_window_height: context
                .and_then(tehuti_protocol::Context::window_height)
                .map(tehuti_core::layout::PresetSize::Proportion),
            ..Default::default()
        }
    }
}

impl XdgShellHandler for Compositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, _surface: ToplevelSurface) {}

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.answer_state_request(&surface, None);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.answer_state_request(&surface, None);
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        self.answer_state_request(&surface, Some(true));
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.answer_state_request(&surface, Some(false));
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.sync_destination_windows();
        self.update_focus(&surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if self.update_helper(&surface) {
            self.place_layout();
        }
        self.sync_destination_windows();
        if let Some(Event::Focus { app_id, title }) = self.focus_event(&surface) {
            self.speech_title.clone_from(&title);
            let _rejection = self.dispatch(DispatchInput::FocusTitle { app_id, title });
            self.refresh_speech_vocabulary();
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let closing = Some(surface.wl_surface());
        if self.window_focus.as_ref().map(|(focused, _)| focused) == closing
            || self.unfocused_window.as_ref() == closing
        {
            self.unfocused_window = None;
            let _rejection = self.dispatch(DispatchInput::FocusedWindowClosed);
        }
        self.remove_layout_window(surface.wl_surface(), false);
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
    /// Applies the decorations setting (default `none`). With [`Decorations::None`](crate::decorations::Decorations::None)
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
        tracing::debug!(target: "pad_latency", stage = "window_focus", at_us = crate::controller::timestamp_us(std::time::SystemTime::now()));
        set_data_device_focus(
            &self.display,
            seat,
            focused.and_then(smithay::reexports::wayland_server::Resource::client),
        );
        self.text_input.focus(focused.cloned());
        self.taps.focus_changed();
        // smithay holds the keyboard lock here; a dispatch can relayout and call
        // `set_focus` again, which deadlocks. Defer it until the lock is released.
        self.focus_change_dispatch = Some(DispatchInput::TextField {
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

/// An action for the log, with typed and noticed text reduced to its length.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other action carries no free text"
)]
fn redacted_action(action: &Action) -> String {
    match action {
        Action::Text { text } => format!("Text({} bytes)", text.as_str().len()),
        Action::Notice { text } => format!("Notice({} bytes)", text.as_str().len()),
        other => format!("{other:?}"),
    }
}

fn log_speech_route(text: &tehuti_protocol::Text, first: Option<&Action>, plugin: bool) {
    match first {
        Some(Action::Notice { text: notice }) if notice.as_str() == "NO SPEECH MAPPING" => {
            tracing::info!(bytes = text.as_str().len(), "speech has no mapping");
        }
        Some(action) => {
            let shown = redacted_action(action);
            tracing::info!(bytes = text.as_str().len(), action = shown, "speech mapped");
        }
        None if plugin => tracing::info!(bytes = text.as_str().len(), "speech routed to a plugin"),
        None => tracing::info!(bytes = text.as_str().len(), "speech produced no action"),
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
    tracing::info!(session, bytes = text.as_str().len(), "speech transcript received");
    if !admitted {
        tracing::info!("speech transcript dropped for a stale session");
    }
}

impl Compositor {
    fn volume_request(&mut self, request: crate::volume::Request) {
        if self.volume.is_none() {
            match crate::volume::Backend::start() {
                Ok(backend) => self.volume = Some(backend),
                Err(error) => {
                    tracing::warn!(%error, "audio worker unavailable");
                    let _rejection = self.dispatch(DispatchInput::Volume(None));
                    self.show_notice("VOLUME UNAVAILABLE");
                    return;
                }
            }
        }
        if let Some(backend) = &self.volume
            && let Err(error) = backend.request(request)
        {
            tracing::warn!(%error, "audio worker unavailable");
            self.volume = None;
            let _rejection = self.dispatch(DispatchInput::Volume(None));
            self.show_notice("VOLUME UNAVAILABLE");
        }
    }

    fn poll_volume(&mut self) {
        let replies: Vec<_> = self
            .volume
            .as_ref()
            .map_or_else(Vec::new, |backend| backend.poll().collect());
        for reply in replies {
            match reply {
                crate::volume::Reply::Read(result) => {
                    if let Err(error) = &result {
                        tracing::warn!(%error, "volume read failed");
                        self.show_notice("VOLUME UNAVAILABLE");
                    }
                    let _rejection = self.dispatch(DispatchInput::Volume(result.ok()));
                }
                crate::volume::Reply::Set(Err(error)) => {
                    tracing::warn!(%error, "volume write failed");
                    self.show_notice("VOLUME UNAVAILABLE");
                }
                crate::volume::Reply::Set(Ok(())) => {}
            }
        }
    }
}

/// The `app_id` a toplevel declared, if any.
fn toplevel_app_id(surface: &ToplevelSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
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
    })
}

#[cfg(test)]
#[path = "state_focus_tests.rs"]
mod focus_tests;

#[cfg(test)]
mod tests {
    use super::Compositor;
    use core::time::Duration;
    use smithay::reexports::wayland_server::Display;
    use tehuti_protocol::Event;

    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn info_logs_carry_the_transcript_length_not_the_text() {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer({
                let capture = capture.clone();
                move || capture.clone()
            })
            .finish();
        let secret = tehuti_protocol::Text::new("zebra password hunter2".into()).unwrap();
        tracing::subscriber::with_default(subscriber, || {
            super::log_transcript(1, &secret, true);
            super::log_speech_route(&secret, None, false);
            super::log_speech_route(&secret, None, true);
            let action = tehuti_protocol::Action::Text { text: secret.clone() };
            super::log_speech_route(&secret, Some(&action), false);
        });
        let logged = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(!logged.contains("hunter2"), "{logged}");
        assert!(logged.contains("bytes=22"), "{logged}");
    }

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

    fn compositor() -> (Display<Compositor>, Compositor) {
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_driver_input(false);
        (display, compositor)
    }

    fn is_down(compositor: &Compositor, code: u32) -> bool {
        compositor
            .keyboard
            .pressed_keys()
            .contains(&smithay::input::keyboard::Keycode::new(code))
    }

    #[test]
    fn tapped_key_is_held_until_the_hold_time_passes() {
        let (_display, mut compositor) = compositor();
        compositor.tap_held(smithay::input::keyboard::Keycode::new(9), &[]);
        assert!(is_down(&compositor, 9));
        assert!(compositor.take_release_request().is_some());

        compositor.advance_clocks(Duration::from_millis(39));
        compositor.release_due_keys();
        assert!(is_down(&compositor, 9));

        compositor.advance_clocks(Duration::from_millis(1));
        compositor.release_due_keys();
        assert!(!is_down(&compositor, 9));
        assert_eq!(compositor.release_timer_fired(), None);
    }

    #[test]
    fn double_tap_is_not_released_by_the_first_hold() {
        let (_display, mut compositor) = compositor();
        let key = smithay::input::keyboard::Keycode::new(9);
        compositor.tap_held(key, &[]);
        compositor.advance_clocks(Duration::from_millis(30));
        compositor.tap_held(key, &[]);
        compositor.advance_clocks(Duration::from_millis(30));
        compositor.release_due_keys();
        assert!(is_down(&compositor, 9));
        compositor.advance_clocks(Duration::from_millis(39));
        compositor.release_due_keys();
        assert!(is_down(&compositor, 9));
        compositor.advance_clocks(Duration::from_millis(1));
        compositor.release_due_keys();
        assert!(!is_down(&compositor, 9));
    }

    #[test]
    fn shared_modifier_stays_down_for_the_later_chord() {
        let (_display, mut compositor) = compositor();
        let control = smithay::input::keyboard::Keycode::new(37);
        compositor.tap_held(smithay::input::keyboard::Keycode::new(38), &[control]);
        compositor.advance_clocks(Duration::from_millis(30));
        compositor.tap_held(smithay::input::keyboard::Keycode::new(39), &[control]);
        compositor.advance_clocks(Duration::from_millis(10));
        compositor.release_due_keys();
        assert!(!is_down(&compositor, 38));
        assert!(is_down(&compositor, 39));
        assert!(is_down(&compositor, 37));
        compositor.advance_clocks(Duration::from_millis(30));
        compositor.release_due_keys();
        assert!(!is_down(&compositor, 39));
        assert!(!is_down(&compositor, 37));
    }

    #[test]
    fn focus_change_releases_held_keys() {
        let (_display, mut compositor) = compositor();
        compositor.tap_held(smithay::input::keyboard::Keycode::new(9), &[]);
        compositor.taps.focus_changed();
        compositor.focus_change_dispatch = Some(super::DispatchInput::TextField {
            active: false,
            changed: false,
            kind: tehuti_core::keyboard::FieldKind::Normal,
        });
        compositor.flush_focus_change();
        assert!(!is_down(&compositor, 9));
    }

    #[test]
    fn pending_focus_flush_keeps_a_modifier_pushed_after_the_change() {
        let (_display, mut compositor) = compositor();
        compositor.taps.focus_changed();
        compositor.focus_change_dispatch = Some(super::DispatchInput::TextField {
            active: false,
            changed: false,
            kind: tehuti_core::keyboard::FieldKind::Normal,
        });
        let control = smithay::input::keyboard::Keycode::new(37);
        compositor.tap_held(smithay::input::keyboard::Keycode::new(38), &[control]);
        assert!(is_down(&compositor, 37));
        assert!(is_down(&compositor, 38));
    }

    #[test]
    fn retap_leaves_a_gap_before_the_next_press() {
        let (_display, mut compositor) = compositor();
        let key = smithay::input::keyboard::Keycode::new(9);
        compositor.tap_held(key, &[]);
        compositor.advance_clocks(Duration::from_millis(5));
        compositor.tap_held(key, &[]);
        assert!(!is_down(&compositor, 9));
        compositor.advance_clocks(tehuti_core::tap_hold::RETAP_GAP);
        compositor.release_due_keys();
        assert!(is_down(&compositor, 9));
    }

    #[test]
    fn failed_timer_releases_at_once_and_stays_idle() {
        let (_display, mut compositor) = compositor();
        compositor.tap_held(smithay::input::keyboard::Keycode::new(9), &[]);
        assert!(compositor.take_release_request().is_some());
        compositor.release_timer_failed();
        assert!(!is_down(&compositor, 9));
        compositor.tap_held(smithay::input::keyboard::Keycode::new(9), &[]);
        assert!(compositor.take_release_request().is_some());
    }
}
