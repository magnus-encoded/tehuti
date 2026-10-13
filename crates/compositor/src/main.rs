//! Tehuti nested in the current desktop: one application fullscreen, driven by the controller.
#![expect(
    clippy::multiple_crate_versions,
    reason = "Smithay and winit pull in duplicate transitive versions outside our control"
)]

mod nested;

use core::{error::Error, time::Duration};
use std::{os::fd::AsFd, process::Command, sync::Arc};

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
}

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let mut application = std::env::args_os().skip(1);
    let context_directory = application
        .next()
        .ok_or("usage: tehuti <context-directory> <application> [arguments...]")?;
    let program = application
        .next()
        .ok_or("usage: tehuti <context-directory> <application> [arguments...]")?;

    let mut event_loop: EventLoop<LoopData> = EventLoop::try_new()?;
    let mut display: Display<Compositor> = Display::new()?;
    let mut compositor = Compositor::new(&display.handle())?;
    compositor.set_contexts(tehuti::contexts::load(std::path::Path::new(&context_directory)));
    let plugin_directory = std::path::Path::new(&context_directory).join("plugins");
    if plugin_directory.exists() {
        compositor.set_plugins(tehuti::plugins::install(
            &event_loop,
            &plugin_directory,
            |data| &mut data.compositor,
        )?);
    }
    if let Some(speech_program) = std::env::var_os("TEHUTI_SPEECH_SIDECAR") {
        compositor.set_speech(tehuti::speech::SpeechProcess::new(vec![speech_program]));
    }
    let socket_name = insert_wayland_sources(&event_loop, &mut display)?;
    compositor.set_launch_socket(socket_name.clone());
    let controller = Some(insert_controller(&event_loop)?);
    nested::insert(&event_loop, &display.handle(), &mut compositor)?;

    let _child = Command::new(program)
        .args(application)
        .env("WAYLAND_DISPLAY", &socket_name)
        .spawn()?;

    let mut data = LoopData {
        display,
        compositor,
        controller,
    };
    let signal = event_loop.get_signal();
    event_loop.run(Some(Duration::from_millis(10)), &mut data, |data| {
        advance_time(data);
        if let Some(pad) = data.controller.as_mut() {
            if let Err(error) = input::finish_quit(&data.compositor, pad, &signal) {
                error!(%error, "controller release failed");
            }
        } else if data.compositor.quit_requested() {
            signal.stop();
        }
    })?;
    Ok(())
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

fn insert_controller(event_loop: &EventLoop<LoopData>) -> Result<EvdevPad, Box<dyn Error>> {
    let device = controller::open_grabbed().ok_or("no controller found: Tehuti needs a pad it can grab")?;
    info!(name = device.name(), "controller grabbed");
    let controller_fd = device.as_fd().try_clone_to_owned()?;
    let _controller_token = event_loop.handle().insert_source(
        Generic::new(controller_fd, Interest::READ, calloop::Mode::Level),
        |_, _, data| Ok(route_controller_input(data)),
    )?;
    Ok(device)
}

/// Routes ready controller input. A controller that fails to read is dropped with its event source.
fn route_controller_input(data: &mut LoopData) -> PostAction {
    advance_time(data);
    let Some(device) = data.controller.as_mut() else {
        return PostAction::Remove;
    };
    match input::route(&mut data.compositor, device) {
        Ok(()) => PostAction::Continue,
        Err(error) => {
            error!(%error, "controller read failed; dropping it");
            data.controller = None;
            PostAction::Remove
        }
    }
}

fn advance_time(data: &mut LoopData) {
    data.compositor.advance_time();
}
