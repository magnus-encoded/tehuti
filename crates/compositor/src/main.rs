//! Tehuti on a seat or nested in the current desktop, driven by the controller.
#![expect(
    clippy::multiple_crate_versions,
    reason = "Smithay and winit pull in duplicate transitive versions outside our control"
)]

mod nested;
mod segv_backtrace;
mod tty;

use core::{error::Error, time::Duration};
use std::{collections::BTreeMap, os::fd::AsFd, path::PathBuf, process::ExitCode, sync::Arc};

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
    /// The virtual pad of an unplugged controller, kept so the application never loses it.
    spare_virtual_pad: Option<controller::VirtualPad>,
    pad_gate: input::PadGate,
    /// Keys each attached pad; a later pad never changes the key of one already there.
    pads: tehuti_core::pad::Pads,
    virtual_pad: Option<String>,
    /// The pad that sent the latest admitted input; only it is mirrored to a virtual pad.
    /// Unplugging it does not clear this: no pad matches the stale path, so no mirror exists
    /// until another pad sends input and takes over.
    active_pad: Option<PathBuf>,
    controller_tokens: BTreeMap<PathBuf, calloop::RegistrationToken>,
    rescan_controller: bool,
    discovery: controller::Discovery,
    discovery_epoch: std::time::Instant,
    seat_active: bool,
    log_out: bool,
    redraw: Option<Redraw>,
    pacing: LoopPacing,
    context_watch: Option<tehuti::contexts::ContextWatch>,
    applications_watch: Option<tehuti::applications_watch::ApplicationsWatch>,
    menu_watch: Option<tehuti::menu_watch::MenuWatch>,
    driver: Option<tehuti::driver::Driver>,
    control: Option<tehuti::ctl::Server>,
    control_directory: PathBuf,
    control_mode: tehuti::output_mode::Temporary<smithay::output::Mode>,
    frames: Frames,
}

#[derive(Debug)]
enum Frames {
    Backend,
    Headless,
    /// Every DRM frame failed for a minute; the session exits with an error.
    Failed,
}

/// Draws a frame when something on the output changed; the DRM backend installs it.
pub(crate) struct Redraw {
    draw: Box<dyn FnMut(&mut LoopData)>,
    shutdown: BackendShutdown,
}

type BackendShutdown = Box<dyn FnOnce(&mut EventLoop<LoopData>, bool)>;

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
        if matches!(self.frames, Frames::Failed) {
            return ExitCode::FAILURE;
        }
        exit_code(
            self.log_out || self.compositor.log_out_requested(),
            self.compositor.quit_requested(),
            self.compositor.restart_requested(),
        )
    }
}

fn exit_code(log_out: bool, quit_requested: bool, restart_requested: bool) -> ExitCode {
    ExitCode::from(if restart_requested {
        75
    } else if log_out {
        21
    } else if quit_requested {
        20
    } else {
        0
    })
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
        "{}",
        tehuti::cli::format_version(
            option_env!("TEHUTI_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
            env!("TEHUTI_GIT_HASH"),
            env!("TEHUTI_BUILD_DATE")
        )
    )?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty());
    writeln!(
        std::io::stdout().lock(),
        "{}",
        tehuti::status::gather(
            runtime.as_deref().map(std::path::Path::new),
            std::path::Path::new("/proc"),
            std::path::Path::new("/dev/dri"),
        )
    )?;
    Ok(ExitCode::SUCCESS)
}

fn default_config_directory() -> Result<PathBuf, Box<dyn Error>> {
    Ok(std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or("neither XDG_CONFIG_HOME nor HOME is set")?
        .join("tehuti"))
}

const CONFIG_USAGE: &str = "usage: tehuti config share <context> <section>... [--as <name>]";

/// `tehuti config share`: moves a section of a context into a shared section.
fn config_command(arguments: &[std::ffi::OsString]) -> Result<ExitCode, Box<dyn Error>> {
    use std::io::Write;
    let words: Vec<&str> = arguments
        .iter()
        .map(|word| word.to_str().ok_or(CONFIG_USAGE))
        .collect::<Result<_, _>>()?;
    let (context, sections, name) = match words.as_slice() {
        ["share", context, rest @ ..] => match rest {
            [sections @ .., "--as", name] if !sections.contains(&"--as") => (*context, sections, Some(*name)),
            sections if !sections.contains(&"--as") => (*context, sections, None),
            _ => return Err(CONFIG_USAGE.into()),
        },
        _ => return Err(CONFIG_USAGE.into()),
    };
    if sections.is_empty() {
        return Err(CONFIG_USAGE.into());
    }
    let report = tehuti::shared_sections::share(&default_config_directory()?, context, sections, name)?;
    writeln!(std::io::stdout().lock(), "{report}")?;
    Ok(ExitCode::SUCCESS)
}

fn check_config(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<ExitCode, Box<dyn Error>> {
    use std::io::Write;
    let directory = match arguments.next() {
        Some(directory) => PathBuf::from(directory),
        None => default_config_directory()?,
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

fn install_skill(path: Option<std::ffi::OsString>) -> Result<ExitCode, Box<dyn Error>> {
    use std::io::Write;
    let root = match path {
        Some(path) => PathBuf::from(path),
        None => tehuti::skill_install::default_root(&PathBuf::from(
            std::env::var_os("HOME").ok_or("HOME is not set")?,
        )),
    };
    let done = tehuti::skill_install::install(&root, env!("TEHUTI_GIT_HASH"))?;
    let verb = if done.updated { "updated" } else { "installed" };
    writeln!(
        std::io::stdout().lock(),
        "{verb} {}\n{verb} {}",
        done.skill.display(),
        done.openai_yaml.display()
    )?;
    Ok(ExitCode::SUCCESS)
}

fn insert_backend(
    event_loop: &EventLoop<'static, LoopData>,
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
/// Another process holds the DRM device; restarting cannot help until it is closed.
const EXIT_DEVICE_BUSY: u8 = 4;

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
    let mut arguments = std::env::args_os().skip(1).peekable();
    let headless = arguments
        .peek()
        .is_some_and(|argument| argument == "--test-driver");
    if headless {
        let _flag = arguments.next();
    }
    let (context_directory, program, application) = match tehuti::cli::parse(arguments) {
        Ok(tehuti::cli::Invocation::Ctl(arguments)) => {
            tehuti::ctl_cli::stdout(arguments)?;
            return Ok(ExitCode::SUCCESS);
        }
        Ok(tehuti::cli::Invocation::Help) => return print_usage(),
        Ok(tehuti::cli::Invocation::Version) => return print_version(),
        Ok(tehuti::cli::Invocation::CheckConfig(rest)) => return check_config(rest.into_iter()),
        Ok(tehuti::cli::Invocation::Config(rest)) => return config_command(&rest),
        Ok(tehuti::cli::Invocation::InstallSkill(path)) => return install_skill(path),
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
    let backend = if headless {
        tehuti::backend::Backend::Nested
    } else {
        tehuti::backend::select(
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
            std::env::var_os("DISPLAY").as_deref(),
        )
    };
    let _instance = match tehuti::instance::acquire_from_env(backend) {
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
    match start_session(&context_directory, program, application, headless, backend) {
        Err(error) if error.is::<tehuti::drm_holders::DeviceBusy>() => {
            error!("{error}");
            refuse(&error, EXIT_DEVICE_BUSY)
        }
        result => result,
    }
}

#[expect(clippy::too_many_lines, reason = "speech spawn stays in startup")]
fn start_session(
    context_directory: &std::ffi::OsStr,
    program: Option<std::ffi::OsString>,
    application: Vec<std::ffi::OsString>,
    headless: bool,
    backend: tehuti::backend::Backend,
) -> Result<ExitCode, Box<dyn Error>> {
    initialize_tracing();
    segv_backtrace::install_if_requested()?;

    let mut event_loop: EventLoop<LoopData> = EventLoop::try_new()?;
    let mut driver = if backend == tehuti::backend::Backend::Nested {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("test driver requires XDG_RUNTIME_DIR")?;
        Some(tehuti::driver::Driver::bind(
            std::path::Path::new(&runtime),
            backend,
        )?)
    } else {
        None
    };
    if std::env::var_os("TEHUTI_SAY").is_some_and(|value| value == "1") {
        let fifo = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "TEHUTI_SAY=1 requires XDG_RUNTIME_DIR",
                )
            })
            .and_then(|runtime| tehuti::say::SayFifo::create(std::path::Path::new(&runtime)));
        match fifo {
            Err(error) => warn!(%error, "typed speech FIFO unavailable"),
            Ok(fifo) => {
                let say_source = event_loop.handle().insert_source(
                    Generic::new(fifo, Interest::READ, calloop::Mode::Level),
                    |_, fifo, data| {
                        match fifo.drain() {
                            Ok(lines) => {
                                for text in lines {
                                    data.compositor.say(text);
                                    data.pacing.input_arrived = true;
                                }
                            }
                            Err(error) => {
                                warn!(%error, "typed speech FIFO failed");
                                return Ok(PostAction::Remove);
                            }
                        }
                        Ok(PostAction::Continue)
                    },
                );
                if let Err(error) = say_source {
                    warn!(%error, "typed speech FIFO unavailable");
                }
            }
        }
    }
    let mut display: Display<Compositor> = Display::new()?;
    let mut compositor = Compositor::new(&display.handle())?;
    compositor.set_timeline_level(tehuti::timeline_level::TimelineLevel::load());
    match tehuti::display_mode_store::config_root() {
        Ok(root) => compositor.load_bar_font(tehuti::display_mode_store::Store::new(
            root.join("tehuti/bar-font"),
        )),
        Err(error) => warn!(%error, "bar font will not be saved"),
    }
    compositor.enable_invitation();
    compositor.set_decorations(tehuti::decorations::from_env());
    compositor.configure_destinations(tehuti::destinations::Paths {
        config: std::path::PathBuf::from(context_directory),
        ..tehuti::destinations::Paths::default()
    });
    compositor.set_context_files(tehuti::contexts::load_files(std::path::Path::new(
        context_directory,
    )));
    match tehuti::input_choice_store::Store::from_env() {
        Ok(store) => compositor.load_input_choices(store),
        Err(error) => warn!(%error, "input choices will not be saved"),
    }
    if driver.is_none()
        && let Some(speech_program) = std::env::var_os("TEHUTI_SPEECH_SIDECAR")
    {
        compositor.set_speech(tehuti::speech::SpeechProcess::new(vec![speech_program]));
    }
    let virtual_pad = std::env::var("TEHUTI_VIRTUAL_PAD").ok();
    let socket_name = insert_wayland_sources(&event_loop, &mut display)?;
    if driver.is_none() {
        install_xwayland(&event_loop, &socket_name)?;
    } else {
        tehuti::child_process::configure_display(None)?;
    }
    install_plugins(
        &event_loop,
        std::path::Path::new(context_directory),
        &mut compositor,
    )?;
    compositor.set_launch_socket(socket_name.clone());
    if driver.is_none() {
        restore_session(&mut compositor, &event_loop);
    }
    if driver.is_none()
        && let Err(error) = insert_controller_watch(&event_loop)
    {
        warn!(%error, "controller watch unavailable; periodic discovery continues");
    }
    let redraw = if headless {
        tehuti::driver::headless_output(&display.handle(), &mut compositor)?;
        None
    } else {
        insert_backend(&event_loop, &display.handle(), &mut compositor)?
    };
    if let Some(driver) = driver.as_mut() {
        let _scene = driver.execute(&mut compositor, tehuti::driver::Request::Scene)?;
    }
    let mut controllers = BTreeMap::new();
    let mut controller_tokens = BTreeMap::new();
    if driver.is_none() {
        insert_controllers(
            &event_loop.handle(),
            virtual_pad.as_deref(),
            &mut controllers,
            &mut controller_tokens,
        );
    }
    let discovery = if driver.is_some() {
        controller::Discovery::Attached {
            retry_at: Duration::ZERO,
        }
    } else {
        initial_discovery(!controllers.is_empty(), &mut compositor)
    };

    // Only the real session registers; nested runs must not touch the host's session target.
    let _registration = (backend == tehuti::backend::Backend::Tty && !headless).then(|| {
        tehuti::session_registration::Registration::start(
            &socket_name,
            tehuti::child_process::display(),
            &mut |warning| warn!(%warning, "systemd session registration failed"),
        )
    });
    let child = program
        .map(|program| {
            tehuti::client_env::command(&program, &socket_name)
                .args(application)
                .spawn()
        })
        .transpose()?;

    let applications_watch = insert_applications_watch(&event_loop, &compositor.destination_paths().data);
    let menu_watch = insert_menu_watch(&event_loop);
    install_speech_resolver(&event_loop, &mut compositor);
    let mut data = LoopData {
        applications_watch,
        menu_watch,
        display,
        compositor,
        controllers,
        spare_virtual_pad: None,
        pad_gate: input::PadGate::default(),
        pads: tehuti_core::pad::Pads::default(),
        controller_tokens,
        virtual_pad,
        active_pad: None,
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
        control: match tehuti::ctl::socket_path().and_then(tehuti::ctl::Server::bind) {
            Ok(server) => Some(server),
            Err(error) => {
                warn!(%error, "session control unavailable");
                None
            }
        },
        control_directory: PathBuf::from(context_directory),
        control_mode: tehuti::output_mode::Temporary::default(),
        driver,
        frames: if headless {
            Frames::Headless
        } else {
            Frames::Backend
        },
        context_watch: insert_context_watch(&event_loop, std::path::Path::new(context_directory)),
    };
    sync_controller_names(&mut data);
    data.compositor.advance_time();
    let result = run(&mut event_loop, &mut data, child);
    let code = data.exit_code();
    if data.driver.is_none()
        && tehuti::session_restore::save_due(
            result.is_ok(),
            data.compositor.quit_requested(),
            tehuti::session_restore::enabled(),
        )
    {
        save_session(&data.compositor);
    }
    shut_down(event_loop, data);
    result?;
    Ok(code)
}

fn restore_session(compositor: &mut tehuti::state::Compositor, event_loop: &EventLoop<LoopData>) {
    use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
    if !tehuti::session_restore::enabled() {
        return;
    }
    let taken = tehuti::session_restore::Store::from_env()
        .map_err(|error| error.to_string())
        .and_then(|store| store.take().map_err(|error| error.to_string()));
    match taken {
        Ok(Some(saved)) => {
            compositor.begin_session_restore(&saved);
            let timer = Timer::from_duration(tehuti::session_restore::PLACEMENT_LIFETIME);
            let armed = event_loop.handle().insert_source(timer, |_, (), data| {
                data.compositor.advance_time();
                TimeoutAction::Drop
            });
            if let Err(error) = armed {
                warn!(%error, "session restore expiry timer unavailable");
            }
        }
        Ok(None) => {}
        Err(error) => warn!(%error, "session restore skipped"),
    }
}

fn save_session(compositor: &tehuti::state::Compositor) {
    let saved = compositor.session_to_save();
    let result = tehuti::session_restore::Store::from_env().and_then(|store| store.save(&saved));
    if let Err(error) = result {
        warn!(%error, "session not saved");
    }
}

fn install_xwayland(
    event_loop: &EventLoop<LoopData>,
    socket: &std::ffi::OsStr,
) -> Result<(), Box<dyn Error>> {
    use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
    let satellite = tehuti::xwayland::discover();
    tehuti::child_process::configure_display(satellite.as_ref().map(|(_, display)| display.into()))?;
    if let Some((binary, display)) = satellite {
        let mut process = tehuti::xwayland::Satellite::new(binary, display, socket.to_owned());
        process.poll(core::time::Duration::ZERO);
        let interval = core::time::Duration::from_millis(100);
        let mut previous = std::time::Instant::now();
        let mut pending_ticks = 0_u32;
        let mut given_up: Option<String> = None;
        let _token = event_loop
            .handle()
            .insert_source(Timer::from_duration(interval), move |_, (), data| {
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(previous);
                process.poll(elapsed);
                let keymap = data.compositor.keymap();
                if !process.is_running() {
                    // A restarted satellite starts blank: always sync it afresh.
                    pending_ticks = 0;
                    given_up = None;
                }
                if given_up.as_deref() != Some(keymap) {
                    given_up = None;
                    if process.sync_keymap(elapsed, keymap) {
                        pending_ticks = 0;
                    } else {
                        // Log once per pending sync; give up after about a minute.
                        if pending_ticks == 0 {
                            tracing::debug!("Xwayland keymap synchronization pending");
                        }
                        pending_ticks = pending_ticks.saturating_add(1);
                        if pending_ticks >= 600 {
                            tracing::warn!("Xwayland keymap synchronization abandoned");
                            given_up = Some(keymap.to_owned());
                            pending_ticks = 0;
                        }
                    }
                }
                previous = now;
                TimeoutAction::ToDuration(interval)
            })
            .map_err(|error| std::io::Error::other(error.to_string()))?;
    }
    Ok(())
}

/// The nested renderer unbinds EGL from the `wl_display` when dropped, and the
/// compositor's dmabuf importer shares that renderer. Both must go while the
/// display is alive, or the NVIDIA EGL driver frees display state twice.
fn shut_down(mut event_loop: EventLoop<LoopData>, data: LoopData) {
    let LoopData {
        display,
        compositor,
        redraw,
        seat_active,
        ..
    } = data;
    // The dmabuf importer holds a renderer reference. Release it before the
    // backend, while the Wayland display is still alive.
    drop(compositor);
    if let Some(Redraw { draw, shutdown }) = redraw {
        drop(draw);
        shutdown(&mut event_loop, seat_active);
    }
    drop(event_loop);
    drop(display);
}

/// Only the DRM session is watched: a frozen nested window can just be closed.
/// The heartbeat ends with `run`, so shutdown is not watched: an abort there
/// would turn a log-out into a failure.
fn start_watchdog(data: &LoopData) -> Option<tehuti::watchdog::Heartbeat> {
    let _drm = data.redraw.as_ref()?;
    tehuti::watchdog::spawn(tehuti::watchdog::FROZEN_AFTER, tehuti::watchdog::abort_frozen)
        .inspect_err(|error| warn!(%error, "watchdog failed to start"))
        .ok()
}

fn poll_test_session(data: &mut LoopData, signal: &calloop::LoopSignal) {
    if data.driver.is_some() {
        poll_nested_control(data);
    }
    if let Some(driver) = data.driver.as_mut() {
        driver.poll(&mut data.compositor);
        if driver.stopped() {
            signal.stop();
        }
    }
}

fn run(
    event_loop: &mut EventLoop<LoopData>,
    data: &mut LoopData,
    mut child: Option<std::process::Child>,
) -> Result<(), Box<dyn Error>> {
    let termination = tehuti::shutdown::Termination::register()?;
    let signal = event_loop.get_signal();
    let handle = event_loop.handle();
    let heartbeat = start_watchdog(data);
    let result = event_loop.run(Some(tehuti::pacing::LOOP_TIMEOUT), data, |data| {
        heartbeat.iter().for_each(tehuti::watchdog::Heartbeat::beat);
        termination.stop_if_requested(&signal);
        poll_test_session(data, &signal);
        if matches!(data.frames, Frames::Headless) {
            if let Some(output) = data.compositor.output.as_ref() {
                for window in data.compositor.space.elements() {
                    window.send_frame(
                        output,
                        data.compositor.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                }
            }
            data.compositor.space.refresh();
            if let Err(error) = data.display.flush_clients() {
                warn!(%error, "test client flush failed");
            }
        }
        let now = std::time::Instant::now();
        count_wakeup(data, now);
        let input_arrived = core::mem::take(&mut data.pacing.input_arrived);
        let work_due = data.pacing.work.due(now);
        if work_due {
            advance_time(data);
            reload_contexts(data);
            reload_desktop_entries(data);
            if data.driver.is_none() {
                reconnect_controller(data, &handle);
            }
        }
        sync_pad_grab(data);
        arm_key_release(data, &handle);
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

/// Arms a one-shot timer that releases tapped keys after their hold time.
fn arm_key_release(data: &mut LoopData, handle: &calloop::LoopHandle<'_, LoopData>) {
    let Some(delay) = data.compositor.take_release_request() else {
        return;
    };
    let timer = calloop::timer::Timer::from_duration(delay);
    let armed = handle.insert_source(timer, |_, (), loop_data| {
        loop_data.compositor.release_timer_fired().map_or(
            calloop::timer::TimeoutAction::Drop,
            calloop::timer::TimeoutAction::ToDuration,
        )
    });
    if armed.is_err() {
        warn!("key release timer failed; releasing now");
        data.compositor.release_timer_failed();
    }
}

/// Runs the backend's redraw, if it has one.
fn redraw(data: &mut LoopData) {
    if let Some(mut redraw) = data.redraw.take() {
        (redraw.draw)(data);
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

/// Drops controllers that asked to reopen, keeping their virtual pad for the next one.
fn drop_reopened_controllers(data: &mut LoopData) {
    let spare = &mut data.spare_virtual_pad;
    data.controllers.retain(|_, pad| {
        if !pad.watchdog.take_reopen() {
            return true;
        }
        if let Some(kept) = pad.pad.detach_virtual_pad() {
            controller::keep_spare(spare, kept);
        }
        false
    });
}

/// Removes the event sources of pads that are gone, so a dead descriptor wakes the loop once.
fn prune_controller_tokens(data: &mut LoopData, handle: &calloop::LoopHandle<'_, LoopData>) {
    data.controller_tokens.retain(|path, token| {
        if data.controllers.contains_key(path) {
            true
        } else {
            handle.remove(*token);
            false
        }
    });
}

fn reconnect_controller(data: &mut LoopData, handle: &calloop::LoopHandle<'_, LoopData>) {
    if !data.seat_active || data.compositor.quit_requested() {
        return;
    }
    let was_attached = !data.controllers.is_empty();
    drop_reopened_controllers(data);
    prune_controller_tokens(data, handle);
    if was_attached && data.controllers.is_empty() {
        controllers_disconnected(data);
    }
    let state = core::mem::replace(
        &mut data.discovery,
        controller::Discovery::Attached {
            retry_at: Duration::ZERO,
        },
    );
    let (state, notification) = controller::scan_discovery(
        state,
        data.discovery_epoch.elapsed(),
        core::mem::take(&mut data.rescan_controller),
        || {
            insert_controllers(
                handle,
                data.virtual_pad.as_deref(),
                &mut data.controllers,
                &mut data.controller_tokens,
            );
            !data.controllers.is_empty()
        },
    );
    data.discovery = state;
    if notification == Some(controller::DiscoveryEffect::Reconnected) {
        data.compositor
            .controller_input(tehuti_core::controller::Input::Reconnected);
    }
    sync_controller_names(data);
}

/// Tells the settings view which pads are connected and forgets the held buttons of gone pads.
fn sync_controller_names(data: &mut LoopData) {
    let devices: Vec<_> = data
        .controllers
        .iter()
        .map(|(path, controller)| {
            let (vendor, product) = controller.pad.usb_id();
            tehuti_core::pad::PadSighting {
                path: path.clone(),
                name: controller.pad.name().to_owned(),
                vendor,
                product,
                uniq: controller.pad.device_identity(),
                origin: controller.pad.connection(),
            }
        })
        .collect();
    let keyed = controller::keyed_controllers(&mut data.pads, devices);
    for (path, effect) in data.pads.take_effects() {
        match effect {
            tehuti_core::pad::PadEffect::ForgetGate => data.pad_gate.forget(&path),
            tehuti_core::pad::PadEffect::ReleaseHeld(buttons) => {
                for name in buttons {
                    data.compositor
                        .controller_input(tehuti_core::controller::Input::Event(
                            tehuti_protocol::Event::Button {
                                name,
                                state: tehuti_protocol::ButtonState::Released,
                            },
                        ));
                }
            }
        }
    }
    data.compositor
        .configure_controllers(keyed.into_iter().map(|(_, info)| info).collect());
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
    let opened = controller::open_grabbed(virtual_pad, &attached);
    for (path, pad) in opened {
        let registration = pad
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| -> Box<dyn Error> { Box::new(error) })
            .and_then(|controller_fd| {
                handle
                    .insert_source(
                        Generic::new(controller_fd, Interest::READ, calloop::Mode::Level),
                        {
                            let remover = handle.clone();
                            move |_, _, data| {
                                let action = route_controller_input(data);
                                prune_controller_tokens(data, &remover);
                                Ok(action)
                            }
                        },
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
    let companion = controller::nunchuk_present()
        || controllers
            .values()
            .any(|controller| !controller.pad.identity().family.is_remote());
    for controller in controllers.values_mut() {
        controller.pad.set_companion(companion);
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

/// Watches desktop entries; a failure leaves them loaded until restart.
fn insert_applications_watch(
    event_loop: &EventLoop<'static, LoopData>,
    data: &[std::path::PathBuf],
) -> Option<tehuti::applications_watch::ApplicationsWatch> {
    tehuti::applications_watch::ApplicationsWatch::insert(
        &event_loop.handle(),
        data,
        |data: &mut LoopData| data.applications_watch.as_mut(),
    )
    .inspect_err(
        |error| warn!(%error, "applications watch unavailable; desktop entries reload only on restart"),
    )
    .ok()
}

/// Starts the speech resolver when a key is set and curl exists. The worker thread posts;
/// its replies come back through a channel the loop drains.
fn install_speech_resolver(event_loop: &EventLoop<'static, LoopData>, compositor: &mut Compositor) {
    use tehuti::speech::{
        resolver::{ApiKey, Curl, OptIn, Resolver, TurnLog},
        resolver_worker::{Link, Worker},
    };
    let Some(key) = ApiKey::from_env() else {
        return;
    };
    let transport = Curl::default();
    if !transport.available() {
        return;
    }
    let Ok(opt_in) = OptIn::from_env() else {
        return;
    };
    let (sender, receiver) = calloop::channel::channel();
    let inserted = event_loop.handle().insert_source(receiver, |event, (), data| {
        if let calloop::channel::Event::Msg(reply) = event {
            data.compositor.resolver_replied(&reply);
            data.pacing.input_arrived = true;
        }
    });
    if let Err(error) = inserted {
        warn!(%error, "speech resolver unavailable");
        return;
    }
    let sweep = event_loop.handle().insert_source(
        calloop::timer::Timer::from_duration(Duration::from_millis(250)),
        |_, (), data| {
            data.compositor.abandon_stale_resolver(
                std::time::Instant::now(),
                tehuti::speech::resolver_worker::HELD_LIMIT,
            );
            calloop::timer::TimeoutAction::ToDuration(Duration::from_millis(250))
        },
    );
    if let Err(error) = sweep {
        warn!(%error, "speech resolver unavailable");
        return;
    }
    match Worker::spawn(Resolver::new(transport, Some(key)), move |reply| {
        sender.send(reply).is_ok()
    }) {
        Ok(worker) => compositor.set_speech_resolver(Link::new(worker, opt_in, TurnLog::from_env().ok())),
        Err(error) => warn!(%error, "speech resolver unavailable"),
    }
}

/// Watches the menu config; a failure leaves the menu loaded until restart.
fn insert_menu_watch(event_loop: &EventLoop<'static, LoopData>) -> Option<tehuti::menu_watch::MenuWatch> {
    let path = tehuti::system_menu::config_path()
        .inspect_err(|error| warn!(%error, "no menu config path; the menu reloads only on restart"))
        .ok()?;
    tehuti::menu_watch::MenuWatch::insert(&event_loop.handle(), &path, |data: &mut LoopData| {
        data.menu_watch.as_mut()
    })
    .inspect_err(|error| warn!(%error, "menu watch unavailable; the menu reloads only on restart"))
    .ok()
}

/// Reloads the system menu once a burst of edits to its config has gone quiet.
fn reload_menu(data: &mut LoopData) {
    if data
        .menu_watch
        .as_mut()
        .is_some_and(|watch| watch.poll(std::time::Instant::now()))
    {
        data.compositor.reload_menu_config(tehuti::system_menu::load());
    }
}

/// Rebuilds destinations once a burst of desktop entry edits has gone quiet, then checks the menu config.
fn reload_desktop_entries(data: &mut LoopData) {
    if data
        .applications_watch
        .as_mut()
        .is_some_and(|watch| watch.poll(std::time::Instant::now()))
    {
        data.compositor.refresh_destinations();
    }
    reload_menu(data);
}

/// Reloads contexts once a burst of edits has gone quiet.
fn reload_contexts(data: &mut LoopData) {
    if let Some(reload) = data
        .context_watch
        .as_mut()
        .and_then(|watch| watch.poll(std::time::Instant::now()))
    {
        data.compositor.apply_reload(reload);
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
/// Stick motion, and trigger motion that crosses no threshold, arrive as events the core never sees,
/// because sticks are sampled on the loop's periodic work; such a read changes nothing and returns at once.
fn route_controller_input(data: &mut LoopData) -> PostAction {
    let was_attached = !data.controllers.is_empty();
    pump_virtual_pads(data);
    let (compositor, gate) = (&data.compositor, &mut data.pad_gate);
    let mut identity = None;
    let keys = &mut data.pads;
    let mut event_controllers = Vec::new();
    let active = &mut data.active_pad;
    let spare = &mut data.spare_virtual_pad;
    let (events, removed) = input::drain_pads_handing_over(
        &mut data.controllers,
        |path, controller, event| {
            let key = keys
                .id_for(path)
                .map_or_else(|| controller.pad.name().to_owned(), |key| key.as_str().to_owned());
            let admitted = gate.admit(path, compositor.controller_chosen(&key), event);
            if admitted {
                if let tehuti_protocol::Event::Button { name, state } = event {
                    keys.on_button(path, *name, *state == tehuti_protocol::ButtonState::Pressed);
                }
                event_controllers.push(key);
                identity = Some(controller.pad.identity());
                *active = Some(path.to_path_buf());
            }
            admitted
        },
        |controller| {
            if let Some(kept) = controller.pad.detach_virtual_pad() {
                controller::keep_spare(spare, kept);
            }
        },
    );
    if !removed.is_empty() {
        sync_controller_names(data);
    }
    if let Some(identity) = identity {
        data.compositor.set_pad_family(identity.family);
    }
    if events.is_empty() && removed.is_empty() {
        data.pacing.pad_reads_without_input = data.pacing.pad_reads_without_input.saturating_add(1);
        return PostAction::Continue;
    }
    data.pacing.input_arrived = true;
    sync_pad_grab(data);
    advance_time(data);
    for (event, name) in events.into_iter().zip(event_controllers) {
        data.compositor.activate_controller(name);
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
    if data.driver.is_some() {
        return;
    }
    let mut sampled = tehuti_core::dispatch::Sticks::default();
    let keys = &data.pads;
    let recentres = data.compositor.take_recentre_requests();
    for (path, controller) in &mut data.controllers {
        let key = keys
            .id_for(path)
            .map_or_else(|| controller.pad.name().to_owned(), |key| key.as_str().to_owned());
        let chosen = data.compositor.controller_chosen(&key);
        let measure = recentres.contains(&key);
        match controller.pad.calibrate(
            data.compositor.controller_deadzone(&key),
            data.compositor.controller_recentre(&key),
            measure,
        ) {
            Ok(Some(rest)) => data.compositor.store_recentre(key.clone(), rest),
            Ok(None) => {}
            Err(error) => warn!(%error, "controller calibration failed"),
        }
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
    let mirrored = data.compositor.virtual_pad();
    for pad in data.controllers.values_mut() {
        let passthrough = data.compositor.pad_passthrough() && (!mirrored || pad.pad.mirror_unavailable());
        if let Err(error) = pad.pad.set_passthrough(passthrough) {
            warn!(%error, "controller ownership change failed; reopening");
            pad.watchdog.request_reopen();
        }
    }
    // Held-button tracking starts over only when the virtual pad first appears; a switch between
    // pads keeps it, so a plugin still gets the release of a button it took on the old pad.
    if mirrored
        && !data
            .controllers
            .values()
            .any(|controller| controller.pad.mirrored())
    {
        data.compositor.reset_virtual_pad();
    }
    let mut pads: Vec<_> = data
        .controllers
        .iter_mut()
        .map(|(path, controller)| (path.as_path(), &mut controller.pad))
        .collect();
    let compositor = &data.compositor;
    let sync = controller::sync_mirrors(
        &mut pads,
        mirrored,
        data.active_pad.as_deref(),
        &mut data.spare_virtual_pad,
        &|button| compositor.virtual_pad_consumed(button),
    );
    for path in &sync.failed {
        if let Some(pad) = data.controllers.get_mut(path) {
            warn!("passing the physical pad through");
            if let Err(grab_error) = pad.pad.set_passthrough(true) {
                warn!(%grab_error, "controller ownership change failed; reopening");
                pad.watchdog.request_reopen();
            }
        }
    }
}

/// Copies physical input to each virtual pad, applying plugin overrides before the core reads it.
fn pump_virtual_pads(data: &mut LoopData) {
    let compositor = &mut data.compositor;
    let spare = &mut data.spare_virtual_pad;
    for controller in data.controllers.values_mut() {
        if let Err(error) = controller
            .pad
            .pump_mirror(&mut |button, state| compositor.virtual_pad_button(button, state))
        {
            // The physical pad is gone; the virtual pad stays, at rest, for the next one.
            warn!(%error, "physical pad lost; keeping the virtual pad");
            if let Some(kept) = controller.pad.detach_virtual_pad() {
                controller::keep_spare(spare, kept);
            }
            controller.watchdog.request_reopen();
        }
    }
}

fn poll_nested_control(data: &mut LoopData) {
    if let Some(mut control) = data.control.take() {
        control.poll(|command| {
            if let tehuti::ctl::Command::DisplayMode { mode: value } = command {
                let request = tehuti::output_mode::Request::parse(&value).ok_or("invalid output mode")?;
                let output = data.compositor.output.clone().ok_or("no active output")?;
                let current = output.current_mode().ok_or("no output mode")?;
                let modes = output.modes();
                let candidates: Vec<_> = modes
                    .iter()
                    .filter_map(|mode| {
                        Some((
                            u16::try_from(mode.size.w).ok()?,
                            u16::try_from(mode.size.h).ok()?,
                            u32::try_from(mode.refresh).ok()?.checked_div(1000)?,
                        ))
                    })
                    .collect();
                let target = match request {
                    tehuti::output_mode::Request::Restore => None,
                    tehuti::output_mode::Request::Set { .. } => Some(
                        *request
                            .closest(&candidates)
                            .and_then(|index| modes.get(index))
                            .ok_or("no supported output modes")?,
                    ),
                };
                data.control_mode.apply(current, target, |mode| {
                    output.change_current_state(Some(mode), None, None, None);
                    data.compositor.fill_output();
                    Ok::<_, String>(())
                })?;
                Ok(serde_json::json!({}))
            } else {
                data.compositor.control(command, &data.control_directory)
            }
        });
        data.control = Some(control);
    }
}

#[cfg(test)]
mod exit_code_tests {
    use super::exit_code;

    #[test]
    fn restart_and_quit_actions_choose_the_session_exit_status() {
        use smithay::reexports::wayland_server::Display;
        use tehuti::state::Compositor;
        use tehuti_protocol::{ButtonName, ButtonState, Event, Text, parse_context};

        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![
            parse_context(
                "app_id = '*'
buttons = {}
[speech]
restart = { restart = true }",
            )
            .unwrap(),
        ]);
        compositor.say(Text::new("restart".into()).unwrap());
        assert!(compositor.quit_requested());
        assert_eq!(
            exit_code(false, compositor.quit_requested(), compositor.restart_requested()),
            std::process::ExitCode::from(75)
        );

        let mut quitting = Compositor::new(&display.handle()).unwrap();
        for name in [ButtonName::Guide, ButtonName::Start] {
            quitting.handle(&Event::Button {
                name,
                state: ButtonState::Pressed,
            });
        }
        quitting.handle(&Event::Tick {
            elapsed: core::time::Duration::from_secs(3),
        });
        assert!(quitting.quit_requested());
        assert_eq!(
            exit_code(false, quitting.quit_requested(), quitting.restart_requested()),
            std::process::ExitCode::from(20)
        );
    }

    #[test]
    fn restart_exits_with_session_restart_status() {
        for (log_out, quit_requested) in [(false, false), (false, true), (true, true)] {
            assert_eq!(
                exit_code(log_out, quit_requested, true),
                std::process::ExitCode::from(75)
            );
        }
    }

    #[test]
    fn session_actions_use_the_launcher_exit_statuses() {
        assert_eq!(exit_code(true, true, false), std::process::ExitCode::from(21));
        assert_eq!(exit_code(false, true, false), std::process::ExitCode::from(20));
    }

    #[test]
    fn external_shutdown_exits_successfully() {
        assert_eq!(exit_code(false, false, false), std::process::ExitCode::SUCCESS);
    }
}
