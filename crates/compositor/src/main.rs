//! Tehuti on a seat or nested in the current desktop, driven by the controller.
#![expect(
    clippy::multiple_crate_versions,
    reason = "Smithay and winit pull in duplicate transitive versions outside our control"
)]

mod nested;
mod tty;

use core::{error::Error, time::Duration};
use std::{
    collections::BTreeMap,
    os::fd::AsFd,
    path::PathBuf,
    process::{Command, ExitCode},
    sync::Arc,
};

use smithay::{
    reexports::{
        calloop::{self, EventLoop, Interest, PostAction, generic::Generic},
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};
use tehuti::{
    controller::{self, EvdevPad},
    input,
    state::{ClientState, Compositor},
};
use tracing::{error, info, warn};

#[derive(Debug)]
pub(crate) struct LoopData {
    display: Display<Compositor>,
    compositor: Compositor,
    controllers: BTreeMap<PathBuf, AttachedController>,
    pad_gate: input::PadGate,
    virtual_pad: Option<String>,
    controller_tokens: BTreeMap<PathBuf, calloop::RegistrationToken>,
    rescan_controller: bool,
    discovery: controller::Discovery,
    discovery_epoch: std::time::Instant,
    seat_active: bool,
    log_out: bool,
    redraw: Option<Redraw>,
    pacing: LoopPacing,
    context_watch: Option<tehuti::contexts::ContextWatch>,
}

/// Draws a frame when something on the output changed; the DRM backend installs it.
pub(crate) struct Redraw(Box<dyn FnMut(&mut LoopData)>);

impl core::fmt::Debug for Redraw {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Redraw")
    }
}

/// When the loop's periodic work and redraws run, with wakeup counters logged at debug level.
#[derive(Debug)]
struct LoopPacing {
    work: tehuti::pacing::Interval,
    /// Set by sources whose input may change the output, so the next loop turn redraws.
    input_arrived: bool,
    turns: u32,
    pad_reads_without_input: u32,
    report: tehuti::pacing::Interval,
}

#[derive(Debug)]
struct AttachedController {
    pad: EvdevPad,
    watchdog: controller::Watchdog,
}

impl input::InputSource for AttachedController {
    fn drain(&mut self) -> std::io::Result<Vec<tehuti_protocol::Event>> {
        let events = self.pad.drain()?;
        if !events.is_empty() {
            self.watchdog = controller::watch(
                core::mem::take(&mut self.watchdog),
                controller::WatchInput::Events,
            )
            .0;
        }
        Ok(events)
    }

    fn release(&mut self) -> std::io::Result<()> {
        self.pad.release()
    }
}

impl LoopData {
    fn exit_code(&self) -> ExitCode {
        exit_code(self.log_out, self.compositor.quit_requested())
    }
}

fn exit_code(log_out: bool, quit_requested: bool) -> ExitCode {
    ExitCode::from(if log_out {
        21
    } else if quit_requested {
        20
    } else {
        0
    })
}

#[cfg(test)]
mod exit_code_tests {
    use super::exit_code;

    #[test]
    fn external_shutdown_exits_successfully() {
        assert_eq!(exit_code(false, false), std::process::ExitCode::SUCCESS);
    }
}

fn initialize_tracing() {
    let default = if std::env::var_os("TEHUTI_TIMING").is_some() {
        "trace"
    } else {
        "info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default)),
        )
        .init();
}

fn print_version() -> Result<ExitCode, Box<dyn Error>> {
    use std::io::Write;
    writeln!(
        std::io::stdout().lock(),
        "tehuti {} ({} {})",
        env!("CARGO_PKG_VERSION"),
        env!("TEHUTI_GIT_HASH"),
        env!("TEHUTI_BUILD_DATE")
    )?;
    Ok(ExitCode::SUCCESS)
}

fn check_config(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<ExitCode, Box<dyn Error>> {
    use std::io::Write;
    let directory = match arguments.next() {
        Some(directory) => PathBuf::from(directory),
        None => std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or("neither XDG_CONFIG_HOME nor HOME is set")?
            .join("tehuti"),
    };
    if arguments.next().is_some() {
        return Err("usage: tehuti --check-config [config-directory]".into());
    }
    let report = tehuti::config_check::check(&directory);
    write!(std::io::stdout().lock(), "{report}")?;
    Ok(if report.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn insert_backend(
    event_loop: &EventLoop<LoopData>,
    display: &smithay::reexports::wayland_server::DisplayHandle,
    compositor: &mut Compositor,
) -> Result<Option<Redraw>, Box<dyn Error>> {
    Ok(
        match tehuti::backend::select(
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
            std::env::var_os("DISPLAY").as_deref(),
        ) {
            tehuti::backend::Backend::Nested => {
                nested::insert(event_loop, display, compositor)?;
                None
            }
            tehuti::backend::Backend::Tty => Some(tty::insert(event_loop, display, compositor)?),
        },
    )
}

/// Exit codes for a start refused before any session setup.
const EXIT_USAGE: u8 = 2;
const EXIT_ALREADY_RUNNING: u8 = 3;

fn print_usage() -> Result<ExitCode, Box<dyn Error>> {
    use std::io::Write;
    write!(std::io::stdout().lock(), "{}", tehuti::cli::USAGE)?;
    Ok(ExitCode::SUCCESS)
}

fn refuse(message: &dyn core::fmt::Display, code: u8) -> Result<ExitCode, Box<dyn Error>> {
    use std::io::Write;
    writeln!(std::io::stderr().lock(), "tehuti: {message}")?;
    Ok(ExitCode::from(code))
}

fn main() -> Result<ExitCode, Box<dyn Error>> {
    let (context_directory, program, application) = match tehuti::cli::parse(std::env::args_os().skip(1)) {
        Ok(tehuti::cli::Invocation::Help) => return print_usage(),
        Ok(tehuti::cli::Invocation::Version) => return print_version(),
        Ok(tehuti::cli::Invocation::CheckConfig(rest)) => return check_config(rest.into_iter()),
        Ok(tehuti::cli::Invocation::Session {
            context_directory,
            application,
            application_arguments,
        }) => (context_directory, application, application_arguments),
        Err(error) => {
            return refuse(
                &format_args!("{error}\n{}", tehuti::cli::USAGE.trim_end()),
                EXIT_USAGE,
            );
        }
    };
    let _instance = match tehuti::instance::acquire_from_env(tehuti::backend::select(
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        std::env::var_os("DISPLAY").as_deref(),
    )) {
        Ok(lock) => lock,
        Err(error) => {
            let code = if matches!(error, tehuti::instance::InstanceError::AlreadyRunning { .. }) {
                EXIT_ALREADY_RUNNING
            } else {
                1
            };
            return refuse(&error, code);
        }
    };
    start_session(&context_directory, program, application)
}

#[expect(clippy::too_many_lines, reason = "speech spawn stays in startup")]
fn start_session(
    context_directory: &std::ffi::OsStr,
    program: Option<std::ffi::OsString>,
    application: Vec<std::ffi::OsString>,
) -> Result<ExitCode, Box<dyn Error>> {
    initialize_tracing();

    let mut event_loop: EventLoop<LoopData> = EventLoop::try_new()?;
    let mut display: Display<Compositor> = Display::new()?;
    let mut compositor = Compositor::new(&display.handle())?;
    compositor.set_decorations(tehuti::decorations::from_env());
    compositor.set_contexts(tehuti::contexts::load(std::path::Path::new(context_directory)));
    install_plugins(
        &event_loop,
        std::path::Path::new(context_directory),
        &mut compositor,
    )?;
    match tehuti::input_choice_store::Store::from_env() {
        Ok(store) => compositor.load_input_choices(store),
        Err(error) => warn!(%error, "input choices will not be saved"),
    }
    if let Some(speech_program) = std::env::var_os("TEHUTI_SPEECH_SIDECAR") {
        compositor.set_speech(tehuti::speech::SpeechProcess::new(vec![speech_program]));
    }
    let virtual_pad = std::env::var("TEHUTI_VIRTUAL_PAD").ok();
    let socket_name = insert_wayland_sources(&event_loop, &mut display)?;
    compositor.set_launch_socket(socket_name.clone());
    if let Err(error) = insert_controller_watch(&event_loop) {
        warn!(%error, "controller watch unavailable; periodic discovery continues");
    }
    let redraw = insert_backend(&event_loop, &display.handle(), &mut compositor)?;
    let mut controllers = BTreeMap::new();
    let mut controller_tokens = BTreeMap::new();
    insert_controllers(
        &event_loop.handle(),
        virtual_pad.as_deref(),
        &mut controllers,
        &mut controller_tokens,
    );
    let discovery = initial_discovery(!controllers.is_empty(), &mut compositor);

    let child = program
        .map(|program| {
            Command::new(program)
                .args(application)
                .env("WAYLAND_DISPLAY", &socket_name)
                .spawn()
        })
        .transpose()?;

    let mut data = LoopData {
        display,
        compositor,
        controllers,
        pad_gate: input::PadGate::default(),
        controller_tokens,
        virtual_pad,
        rescan_controller: true,
        discovery,
        discovery_epoch: std::time::Instant::now(),
        seat_active: true,
        log_out: false,
        redraw,
        pacing: LoopPacing {
            work: tehuti::pacing::Interval::new(tehuti::pacing::LOOP_WORK_PERIOD),
            input_arrived: true,
            turns: 0,
            pad_reads_without_input: 0,
            report: tehuti::pacing::Interval::new(tehuti::pacing::STATISTICS_PERIOD),
        },
        context_watch: insert_context_watch(&event_loop, std::path::Path::new(context_directory)),
    };
    let result = run(&mut event_loop, &mut data, child);
    let code = data.exit_code();
    shut_down(event_loop, data);
    result?;
    Ok(code)
}

/// The nested renderer unbinds EGL from the `wl_display` when dropped, and the
/// compositor's dmabuf importer shares that renderer. Both must go while the
/// display is alive, or the NVIDIA EGL driver frees display state twice.
fn shut_down(event_loop: EventLoop<LoopData>, data: LoopData) {
    drop(event_loop);
    let LoopData {
        display, compositor, ..
    } = data;
    drop(compositor);
    drop(display);
}

fn run(
    event_loop: &mut EventLoop<LoopData>,
    data: &mut LoopData,
    mut child: Option<std::process::Child>,
) -> Result<(), Box<dyn Error>> {
    let termination = tehuti::shutdown::Termination::register()?;
    let signal = event_loop.get_signal();
    let handle = event_loop.handle();
    let result = event_loop.run(Some(tehuti::pacing::LOOP_TIMEOUT), data, |data| {
        termination.stop_if_requested(&signal);
        let now = std::time::Instant::now();
        count_wakeup(data, now);
        let input_arrived = core::mem::take(&mut data.pacing.input_arrived);
        let work_due = data.pacing.work.due(now);
        if work_due {
            advance_time(data);
            reload_contexts(data);
            reconnect_controller(data, &handle);
        }
        sync_pad_grab(data);
        if work_due || input_arrived {
            redraw(data);
        }
        for pad in data.controllers.values_mut() {
            if let Err(error) = input::finish_quit(&data.compositor, pad, &signal) {
                error!(%error, "controller release failed");
            }
        }
        if data.compositor.quit_requested() {
            signal.stop();
        }
    });
    if let Some(child) = child.as_mut() {
        tehuti::shutdown::terminate(child, false);
    }
    result?;
    Ok(())
}

/// Runs the backend's redraw, if it has one.
fn redraw(data: &mut LoopData) {
    if let Some(mut redraw) = data.redraw.take() {
        (redraw.0)(data);
        data.redraw = Some(redraw);
    }
}

fn count_wakeup(data: &mut LoopData, now: std::time::Instant) {
    data.pacing.turns = data.pacing.turns.saturating_add(1);
    if data.pacing.report.due(now) {
        tracing::debug!(
            turns = data.pacing.turns,
            pad_reads_without_input = data.pacing.pad_reads_without_input,
            period = ?tehuti::pacing::STATISTICS_PERIOD,
            "event loop wakeups"
        );
        data.pacing.turns = 0;
        data.pacing.pad_reads_without_input = 0;
    }
}

fn initial_discovery(attached: bool, compositor: &mut Compositor) -> controller::Discovery {
    if attached {
        controller::Discovery::Attached {
            retry_at: Duration::ZERO,
        }
    } else {
        warn!("no controller found; discovery retries every 250 ms");
        compositor.controller_input(tehuti_core::controller::Input::Disconnected);
        controller::Discovery::Searching {
            retry_at: Duration::ZERO,
        }
    }
}

fn reconnect_controller(data: &mut LoopData, handle: &calloop::LoopHandle<'_, LoopData>) {
    if !data.seat_active || data.compositor.quit_requested() {
        return;
    }
    let was_attached = !data.controllers.is_empty();
    data.controllers.retain(|_, pad| !pad.watchdog.take_reopen());
    data.controller_tokens.retain(|path, token| {
        if data.controllers.contains_key(path) {
            true
        } else {
            handle.remove(*token);
            false
        }
    });
    if was_attached && data.controllers.is_empty() {
        controllers_disconnected(data);
    }
    let state = core::mem::replace(
        &mut data.discovery,
        controller::Discovery::Attached {
            retry_at: Duration::ZERO,
        },
    );
    let (state, effect) = controller::discover(
        state,
        controller::DiscoveryInput::Poll {
            now: data.discovery_epoch.elapsed(),
            rescan: core::mem::take(&mut data.rescan_controller),
        },
    );
    data.discovery = state;
    if effect == Some(controller::DiscoveryEffect::Scan) {
        insert_controllers(
            handle,
            data.virtual_pad.as_deref(),
            &mut data.controllers,
            &mut data.controller_tokens,
        );
        if !data.controllers.is_empty() {
            let searching = core::mem::replace(
                &mut data.discovery,
                controller::Discovery::Attached {
                    retry_at: Duration::ZERO,
                },
            );
            let (attached, notification) = controller::discover(searching, controller::DiscoveryInput::Found);
            data.discovery = attached;
            if notification == Some(controller::DiscoveryEffect::Reconnected) {
                data.compositor
                    .controller_input(tehuti_core::controller::Input::Reconnected);
            }
        }
    }
    sync_controller_names(data);
}

/// Tells the settings view which pads are connected and forgets the held buttons of gone pads.
fn sync_controller_names(data: &mut LoopData) {
    let names = data
        .controllers
        .values()
        .map(|controller| controller.pad.name().to_owned())
        .collect();
    data.compositor.configure_controllers(names);
    let controllers = &data.controllers;
    data.pad_gate.retain(|path| controllers.contains_key(path));
}

fn insert_wayland_sources(
    event_loop: &EventLoop<LoopData>,
    display: &mut Display<Compositor>,
) -> Result<std::ffi::OsString, Box<dyn Error>> {
    let socket = ListeningSocketSource::new_auto()?;
    let socket_name = socket.socket_name().to_os_string();
    let _socket_token = event_loop.handle().insert_source(socket, |stream, (), data| {
        if let Err(error) = data
            .display
            .handle()
            .insert_client(stream, Arc::new(ClientState::default()))
        {
            warn!(%error, "client rejected");
        }
    })?;
    let display_fd = display.backend().poll_fd().try_clone_to_owned()?;
    let _display_token = event_loop.handle().insert_source(
        Generic::new(display_fd, Interest::READ, calloop::Mode::Level),
        |_, _, data| {
            let _dispatched = data.display.dispatch_clients(&mut data.compositor)?;
            data.pacing.input_arrived = true;
            Ok(PostAction::Continue)
        },
    )?;
    Ok(socket_name)
}

fn insert_controllers(
    handle: &calloop::LoopHandle<'_, LoopData>,
    virtual_pad: Option<&str>,
    controllers: &mut BTreeMap<PathBuf, AttachedController>,
    tokens: &mut BTreeMap<PathBuf, calloop::RegistrationToken>,
) {
    let attached = controllers.keys().cloned().collect();
    for (path, pad) in controller::open_grabbed(virtual_pad, &attached) {
        let registration = pad
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| -> Box<dyn Error> { Box::new(error) })
            .and_then(|controller_fd| {
                handle
                    .insert_source(
                        Generic::new(controller_fd, Interest::READ, calloop::Mode::Level),
                        |_, _, data| Ok(route_controller_input(data)),
                    )
                    .map_err(|error| -> Box<dyn Error> { Box::new(error) })
            });
        match registration {
            Ok(token) => {
                info!(name = pad.name(), "controller grabbed");
                let _previous_token = tokens.insert(path.clone(), token);
                let _previous_controller = controllers.insert(
                    path,
                    AttachedController {
                        pad,
                        watchdog: controller::Watchdog::default(),
                    },
                );
            }
            Err(error) => warn!(%error, "controller registration failed; retrying"),
        }
    }
}

/// Starts plugins from `<context-directory>/plugins` when it exists.
fn install_plugins(
    event_loop: &EventLoop<'static, LoopData>,
    context_directory: &std::path::Path,
    compositor: &mut Compositor,
) -> Result<(), Box<dyn Error>> {
    let plugin_directory = context_directory.join("plugins");
    if plugin_directory.exists() {
        compositor.set_plugins(tehuti::plugins::install(event_loop, &plugin_directory, |data| {
            &mut data.compositor
        })?);
    }
    Ok(())
}

/// Watches context files; a failure leaves contexts loaded until restart.
fn insert_context_watch(
    event_loop: &EventLoop<'static, LoopData>,
    directory: &std::path::Path,
) -> Option<tehuti::contexts::ContextWatch> {
    tehuti::contexts::ContextWatch::insert(&event_loop.handle(), directory, |data: &mut LoopData| {
        data.context_watch.as_mut()
    })
    .inspect_err(|error| warn!(%error, "context watch unavailable; contexts reload only on restart"))
    .ok()
}

/// Reloads contexts once a burst of edits has gone quiet.
fn reload_contexts(data: &mut LoopData) {
    if let Some(reload) = data
        .context_watch
        .as_mut()
        .and_then(|watch| watch.poll(std::time::Instant::now()))
    {
        data.compositor.reload_contexts(reload);
    }
}

fn insert_controller_watch(event_loop: &EventLoop<LoopData>) -> Result<(), Box<dyn Error>> {
    let watcher = controller::watch_input_directory(std::path::Path::new("/dev/input"))?;
    let _watch_token = event_loop.handle().insert_source(
        Generic::new(watcher, Interest::READ, calloop::Mode::Level),
        |_, watcher, data| {
            let mut buffer = [0_u8; 4096];
            for _batch in 0..16 {
                match rustix::io::read(&*watcher, &mut buffer) {
                    Ok(0) | Err(rustix::io::Errno::WOULDBLOCK) => break,
                    Ok(_) => data.rescan_controller = true,
                    Err(error) => {
                        warn!(%error, "controller watch failed; periodic discovery continues");
                        return Ok(PostAction::Remove);
                    }
                }
            }
            Ok(PostAction::Continue)
        },
    )?;
    Ok(())
}

/// Routes pending events from every pad; failed registrations are removed on the next loop turn.
///
/// Stick and trigger motion arrives as events the core never sees, because sticks are sampled
/// on the loop's periodic work; such a read changes nothing and returns at once.
fn route_controller_input(data: &mut LoopData) -> PostAction {
    let was_attached = !data.controllers.is_empty();
    let (compositor, gate) = (&data.compositor, &mut data.pad_gate);
    let (events, removed) = input::drain_pads_admitted(&mut data.controllers, |path, controller, event| {
        gate.admit(path, compositor.controller_chosen(controller.pad.name()), event)
    });
    if !removed.is_empty() {
        sync_controller_names(data);
    }
    if events.is_empty() && removed.is_empty() {
        data.pacing.pad_reads_without_input = data.pacing.pad_reads_without_input.saturating_add(1);
        return PostAction::Continue;
    }
    data.pacing.input_arrived = true;
    sync_pad_grab(data);
    advance_time(data);
    for event in events {
        data.compositor
            .controller_input(tehuti_core::controller::Input::Event(event));
        if data.compositor.quit_requested() {
            break;
        }
    }
    if was_attached && data.controllers.is_empty() {
        controllers_disconnected(data);
    }
    PostAction::Continue
}

fn controllers_disconnected(data: &mut LoopData) {
    data.compositor
        .speech_input(tehuti_core::speech::SpeechInput::Disconnected);
    data.compositor
        .controller_input(tehuti_core::controller::Input::Disconnected);
    let state = core::mem::replace(
        &mut data.discovery,
        controller::Discovery::Attached {
            retry_at: Duration::ZERO,
        },
    );
    data.discovery = controller::discover(
        state,
        controller::DiscoveryInput::Disconnected {
            now: data.discovery_epoch.elapsed(),
        },
    )
    .0;
}

fn advance_time(data: &mut LoopData) {
    let mut sampled = tehuti_core::dispatch::Sticks::default();
    for controller in data.controllers.values_mut() {
        let chosen = data.compositor.controller_chosen(controller.pad.name());
        match controller.pad.sticks() {
            Ok(sticks) => {
                if chosen && sticks != tehuti_core::dispatch::Sticks::default() {
                    sampled = sticks;
                }
            }
            Err(error) => {
                warn!(%error, "controller stick sample failed");
                controller.watchdog.request_reopen();
            }
        }
        match controller.pad.activity() {
            Ok(fingerprint) => {
                let (watchdog, verdict) = controller::watch(
                    core::mem::take(&mut controller.watchdog),
                    controller::WatchInput::Sample {
                        now: data.discovery_epoch.elapsed(),
                        fingerprint,
                    },
                );
                controller.watchdog = watchdog;
                if verdict == controller::WatchVerdict::Reopen {
                    controller.watchdog.request_reopen();
                }
            }
            Err(error) => {
                warn!(%error, "controller state query failed");
                controller.watchdog.request_reopen();
            }
        }
    }
    data.compositor.sample_sticks(sampled);
    data.compositor.advance_time();
}

fn sync_pad_grab(data: &mut LoopData) {
    for pad in data.controllers.values_mut() {
        if let Err(error) = pad.pad.set_passthrough(data.compositor.pad_passthrough()) {
            warn!(%error, "controller ownership change failed; reopening");
            pad.watchdog.request_reopen();
        }
    }
}
