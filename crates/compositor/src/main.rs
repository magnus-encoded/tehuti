//! Tehuti on a seat or nested in the current desktop, driven by the controller.
#![expect(
    clippy::multiple_crate_versions,
    reason = "Smithay and winit pull in duplicate transitive versions outside our control"
)]

mod nested;
mod tty;

use core::{error::Error, time::Duration};
use std::{os::fd::AsFd, process::ExitCode, sync::Arc};

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
    controller: Option<EvdevPad>,
    controller_token: Option<calloop::RegistrationToken>,
    rescan_controller: bool,
    watchdog: controller::Watchdog,
    discovery: controller::Discovery,
    discovery_epoch: std::time::Instant,
    seat_active: bool,
    log_out: bool,
    context_watch: Option<tehuti::contexts::ContextWatch>,
    escape: tehuti::escape::Escape,
}

impl LoopData {
    fn exit_code(&self) -> ExitCode {
        ExitCode::from(if self.log_out || self.compositor.log_out_requested() {
            21
        } else if self.compositor.quit_requested() {
            20
        } else {
            1
        })
    }
}

fn initialize_tracing() {
    tracing_subscriber::fmt()
        .with_max_level(if std::env::var_os("TEHUTI_TIMING").is_some() {
            tracing::Level::TRACE
        } else {
            tracing::Level::INFO
        })
        .init();
}

fn main() -> Result<ExitCode, Box<dyn Error>> {
    initialize_tracing();
    let escape = tehuti::escape::Escape::spawn()?;
    let mut application = std::env::args_os().skip(1);
    let context_directory = application
        .next()
        .ok_or("usage: tehuti <context-directory> [application] [arguments...]")?;
    let program = application.next();

    let mut event_loop: EventLoop<LoopData> = EventLoop::try_new()?;
    let mut display: Display<Compositor> = Display::new()?;
    let mut compositor = Compositor::new(&display.handle())?;
    compositor.set_contexts(tehuti::contexts::load(std::path::Path::new(&context_directory)));
    install_speech(&mut compositor);
    let socket_name = insert_wayland_sources(&event_loop, &mut display)?;
    compositor.set_launch_socket(socket_name.clone());
    if let Err(error) = insert_controller_watch(&event_loop) {
        warn!(%error, "controller watch unavailable; periodic discovery continues");
    }
    match tehuti::backend::select(
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        std::env::var_os("DISPLAY").as_deref(),
    ) {
        tehuti::backend::Backend::Nested => nested::insert(&event_loop, &display.handle(), &mut compositor)?,
        tehuti::backend::Backend::Tty => tty::insert(&event_loop, &display.handle(), &mut compositor)?,
    }
    let attached_controller = insert_controller(&event_loop.handle(), &escape)?;
    let (controller, controller_token) =
        attached_controller.map_or((None, None), |(device, token)| (Some(device), Some(token)));
    let discovery = initial_discovery(controller.as_ref(), &mut compositor);

    let mut xwayland = tehuti::xwayland::Runtime::new()?;
    xwayland.export_session_environment();
    xwayland.dispatch(&display.handle(), &mut compositor);

    install_plugins(
        &event_loop,
        std::path::Path::new(&context_directory),
        &mut compositor,
    )?;

    let child = program
        .map(|program| {
            tehuti::child_environment::command(program)
                .args(application)
                .env("WAYLAND_DISPLAY", &socket_name)
                .spawn()
        })
        .transpose()?;

    let mut data = LoopData {
        display,
        compositor,
        controller,
        controller_token,
        rescan_controller: true,
        watchdog: controller::Watchdog::default(),
        discovery,
        discovery_epoch: std::time::Instant::now(),
        seat_active: true,
        log_out: false,
        context_watch: insert_context_watch(&event_loop, std::path::Path::new(&context_directory)),
        escape,
    };
    run(&mut event_loop, &mut data, child, &mut xwayland)?;
    Ok(data.exit_code())
}

/// Starts the speech sidecar named by `TEHUTI_SPEECH_SIDECAR`, if any.
fn install_speech(compositor: &mut Compositor) {
    if let Some(speech_program) = std::env::var_os("TEHUTI_SPEECH_SIDECAR") {
        compositor.set_speech(tehuti::speech::SpeechProcess::new(vec![speech_program]));
    }
}

fn run(
    event_loop: &mut EventLoop<LoopData>,
    data: &mut LoopData,
    mut child: Option<std::process::Child>,
    xwayland: &mut tehuti::xwayland::Runtime,
) -> Result<(), Box<dyn Error>> {
    let termination = tehuti::shutdown::Termination::register()?;
    let signal = event_loop.get_signal();
    let handle = event_loop.handle();
    let result = event_loop.run(Some(Duration::from_millis(10)), data, |data| {
        data.escape.beat();
        xwayland.dispatch(&data.display.handle(), &mut data.compositor);
        termination.stop_if_requested(&signal);
        advance_time(data);
        reload_contexts(data);
        reconnect_controller(data, &handle);
        if let Some(pad) = data.controller.as_mut() {
            if let Err(error) = input::finish_quit(&data.compositor, pad, &signal) {
                error!(%error, "controller release failed");
            }
        } else if data.compositor.quit_requested() {
            signal.stop();
        }
    });
    if let Some(child) = child.as_mut() {
        tehuti::shutdown::terminate(child, false);
    }
    result?;
    Ok(())
}

fn initial_discovery(controller: Option<&EvdevPad>, compositor: &mut Compositor) -> controller::Discovery {
    if controller.is_some() {
        controller::Discovery::Attached
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
    if data.watchdog.take_reopen()
        && data.controller.is_some()
        && let Some(token) = drop_controller(data)
    {
        handle.remove(token);
    }
    let state = core::mem::replace(&mut data.discovery, controller::Discovery::Attached);
    let (state, effect) = controller::discover(
        state,
        controller::DiscoveryInput::Poll {
            now: data.discovery_epoch.elapsed(),
            rescan: core::mem::take(&mut data.rescan_controller),
        },
    );
    data.discovery = state;
    if effect == Some(controller::DiscoveryEffect::Scan) {
        match insert_controller(handle, &data.escape) {
            Ok(Some((device, token))) => {
                data.controller = Some(device);
                data.controller_token = Some(token);
                let searching = core::mem::replace(&mut data.discovery, controller::Discovery::Attached);
                let (attached, notification) =
                    controller::discover(searching, controller::DiscoveryInput::Found);
                data.discovery = attached;
                if notification == Some(controller::DiscoveryEffect::Reconnected) {
                    data.compositor
                        .controller_input(tehuti_core::controller::Input::Reconnected);
                }
            }
            Ok(None) => {}
            Err(error) => warn!(%error, "controller registration failed; retrying"),
        }
    }
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
            Ok(PostAction::Continue)
        },
    )?;
    Ok(socket_name)
}

fn insert_controller(
    handle: &calloop::LoopHandle<'_, LoopData>,
    escape: &tehuti::escape::Escape,
) -> Result<Option<(EvdevPad, calloop::RegistrationToken)>, Box<dyn Error>> {
    let Some(device) = controller::open_grabbed() else {
        return Ok(None);
    };
    info!(name = device.name(), "controller grabbed");
    let controller_fd = device.as_fd().try_clone_to_owned()?;
    match device.open_probe() {
        Ok(watched) => escape.set_pad(Some(watched)),
        Err(error) => warn!(%error, "escape thread cannot watch the pad; heartbeat escape only"),
    }
    let controller_token = handle.insert_source(
        Generic::new(controller_fd, Interest::READ, calloop::Mode::Level),
        |_, _, data| Ok(route_controller_input(data)),
    )?;
    Ok(Some((device, controller_token)))
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

/// Routes ready controller input. A controller that fails to read is dropped with its event source.
fn route_controller_input(data: &mut LoopData) -> PostAction {
    advance_time(data);
    let Some(device) = data.controller.as_mut() else {
        return PostAction::Remove;
    };
    match input::route_counted(&mut data.compositor, device) {
        Ok(0) => PostAction::Continue,
        Ok(_) => {
            data.watchdog = controller::watch(
                core::mem::take(&mut data.watchdog),
                controller::WatchInput::Events,
            )
            .0;
            PostAction::Continue
        }
        Err(error) => {
            error!(%error, "controller read failed; dropping it");
            let _token = drop_controller(data);
            PostAction::Remove
        }
    }
}

/// Forgets the pad, releases everything it held and schedules an immediate rescan.
/// Returns the stale event source token for the caller to remove.
fn drop_controller(data: &mut LoopData) -> Option<calloop::RegistrationToken> {
    data.controller = None;
    data.escape.set_pad(None);
    data.watchdog = controller::Watchdog::default();
    data.compositor
        .speech_input(tehuti_core::speech::SpeechInput::Disconnected);
    data.compositor
        .controller_input(tehuti_core::controller::Input::Disconnected);
    let state = core::mem::replace(&mut data.discovery, controller::Discovery::Attached);
    data.discovery = controller::discover(
        state,
        controller::DiscoveryInput::Disconnected {
            now: data.discovery_epoch.elapsed(),
        },
    )
    .0;
    data.rescan_controller = true;
    data.controller_token.take()
}

fn advance_time(data: &mut LoopData) {
    if let Some(pad) = &data.controller {
        match pad.sticks() {
            Ok(sticks) => data.compositor.sample_sticks(sticks),
            Err(error) => {
                data.compositor
                    .sample_sticks(tehuti_core::dispatch::Sticks::default());
                warn!(%error, "controller stick sample failed; re-opening it");
                data.watchdog.request_reopen();
            }
        }
        if let Ok(fingerprint) = pad.activity() {
            let (watchdog, verdict) = controller::watch(
                core::mem::take(&mut data.watchdog),
                controller::WatchInput::Sample {
                    now: data.discovery_epoch.elapsed(),
                    fingerprint,
                },
            );
            data.watchdog = watchdog;
            if verdict == controller::WatchVerdict::Reopen {
                warn!("controller state changes but no events arrive; re-opening it");
                data.watchdog.request_reopen();
            }
        }
    }
    data.compositor.advance_time();
}
