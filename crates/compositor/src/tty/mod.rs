//! A libseat-owned DRM output with GLES rendering and a pixman fallback.

mod render;

use crate::LoopData;
use core::cell::RefCell;
use core::{error::Error, time::Duration};
use smithay::{
    backend::{
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmSurface, PlaneConfig, PlaneState},
        renderer::damage::OutputDamageTracker,
        session::{Event, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, primary_gpu},
    },
    output::{Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device, connector, framebuffer},
        wayland_server::DisplayHandle,
    },
    utils::{DeviceFd, Rectangle, Transform},
};
use std::rc::Rc;
use tehuti::state::Compositor;
use tracing::warn;

struct Tty {
    keyboard: tehuti::backend::EscapeKeyboard,
    session: LibSeatSession,
    libinput: smithay::reexports::input::Libinput,
    surface: DrmSurface,
    device: DrmDevice,
    renderer: render::DrmRendering,
    output: Output,
    damage: OutputDamageTracker,
    next: bool,
    pending: bool,
    modeset: bool,
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "conflicts with the workspace unreachable_pub lint"
)]
pub(crate) fn insert(
    event_loop: &EventLoop<LoopData>,
    display: &DisplayHandle,
    compositor: &mut Compositor,
) -> Result<(), Box<dyn Error>> {
    let (session, notifier) = LibSeatSession::new()?;
    let udev = UdevBackend::new(session.seat())?;
    let path = primary_gpu(session.seat())?
        .or_else(|| udev.device_list().next().map(|(_, path)| path.to_owned()))
        .ok_or("no DRM device on seat")?;
    let (initialized_tty, drm_notifier) = create_output(session, &path, display, compositor)?;
    let tty = Rc::new(RefCell::new(initialized_tty));
    insert_seat(event_loop, notifier, tty.clone())?;
    insert_keyboard(event_loop, &tty)?;
    let flip_tty = tty.clone();
    let flip_stop = event_loop.get_signal();
    let _drm_token = event_loop
        .handle()
        .insert_source(drm_notifier, move |event, _, _| match event {
            DrmEvent::VBlank(_) => flip_tty.borrow_mut().pending = false,
            DrmEvent::Error(error) => {
                warn!(%error, "DRM event failed");
                flip_stop.stop();
            }
        })?;
    let device_id = tty.borrow().device.device_id();
    let device_stop = event_loop.get_signal();
    let _udev_token = event_loop
        .handle()
        .insert_source(udev, move |event, (), _| match event {
            UdevEvent::Changed { device_id: changed } | UdevEvent::Removed { device_id: changed }
                if changed == device_id =>
            {
                warn!("DRM device changed; switching to Plasma");
                device_stop.stop();
            }
            UdevEvent::Added { .. } | UdevEvent::Changed { .. } | UdevEvent::Removed { .. } => {}
        })?;
    insert_wallpaper_redraw(event_loop, tty.clone())?;
    let render_stop = event_loop.get_signal();
    let _timer_token = event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, (), data| {
            let mut backend = tty.borrow_mut();
            if data.seat_active
                && backend.device.is_active()
                && !backend.pending
                && let Err(error) = backend.render(data)
            {
                warn!(%error, "DRM frame failed");
                render_stop.stop();
            }
            TimeoutAction::ToDuration(Duration::from_millis(16))
        })?;
    Ok(())
}

fn insert_wallpaper_redraw(
    event_loop: &EventLoop<LoopData>,
    tty: Rc<RefCell<Tty>>,
) -> Result<(), Box<dyn Error>> {
    let (ping, source) = smithay::reexports::calloop::ping::make_ping()?;
    let redraw_tty = tty;
    let redraw_stop = event_loop.get_signal();
    let _wallpaper_token = event_loop.handle().insert_source(source, move |(), (), data| {
        let mut backend = redraw_tty.borrow_mut();
        if data.seat_active
            && backend.device.is_active()
            && !backend.pending
            && let Err(error) = backend.render(data)
        {
            warn!(%error, "wallpaper frame failed");
            redraw_stop.stop();
        }
    })?;
    tehuti::wallpaper::set_redraw_ping(ping)?;
    Ok(())
}

fn insert_keyboard(event_loop: &EventLoop<LoopData>, tty: &Rc<RefCell<Tty>>) -> Result<(), Box<dyn Error>> {
    let keyboard_tty = tty.clone();
    let keyboard_stop = event_loop.get_signal();
    let input_backend = smithay::backend::libinput::LibinputInputBackend::new(tty.borrow().libinput.clone());
    let _input_token = event_loop
        .handle()
        .insert_source(input_backend, move |event, (), data| {
            use smithay::backend::input::{InputEvent, KeyState, KeyboardKeyEvent};
            if !data.seat_active {
                return;
            }
            if let InputEvent::Keyboard { event } = event {
                let key = u32::from(event.key_code()).saturating_sub(8);
                let escape = keyboard_tty
                    .borrow_mut()
                    .keyboard
                    .key(key, event.state() == KeyState::Pressed);
                match escape {
                    Some(tehuti::backend::Escape::Stop) => {
                        data.log_out = true;
                        keyboard_stop.stop();
                    }
                    Some(tehuti::backend::Escape::ChangeVt(number)) => {
                        if let Err(error) = keyboard_tty.borrow_mut().session.change_vt(number) {
                            warn!(%error, "VT switch failed");
                        }
                    }
                    None => {}
                }
            } else if matches!(event, InputEvent::DeviceRemoved { .. }) {
                keyboard_tty.borrow_mut().keyboard = tehuti::backend::EscapeKeyboard::default();
            }
        })?;
    Ok(())
}

fn insert_seat(
    event_loop: &EventLoop<LoopData>,
    notifier: smithay::backend::session::libseat::LibSeatSessionNotifier,
    seat_tty: Rc<RefCell<Tty>>,
) -> Result<(), Box<dyn Error>> {
    let weak_handle = event_loop.handle().downgrade();
    let stop = event_loop.get_signal();
    let _seat_token = event_loop
        .handle()
        .insert_source(notifier, move |event, (), data| {
            let mut tty = seat_tty.borrow_mut();
            match event {
                Event::PauseSession => {
                    data.seat_active = false;
                    data.controller = None;
                    data.compositor
                        .controller_input(tehuti_core::controller::Input::Disconnected);
                    if let Some(token) = data.controller_token.take()
                        && let Some(handle) = weak_handle.upgrade()
                    {
                        handle.remove(token);
                    }
                    data.discovery = tehuti::controller::Discovery::Searching {
                        retry_at: Duration::ZERO,
                    };
                    tty.keyboard = tehuti::backend::EscapeKeyboard::default();
                    tty.libinput.suspend();
                    tty.device.pause();
                    tty.pending = false;
                }
                Event::ActivateSession => match tty.device.activate(true) {
                    Ok(()) => {
                        if let Err(error) = tty.libinput.resume() {
                            warn!(?error, "keyboard resume failed");
                            stop.stop();
                        }
                        data.seat_active = true;
                        tty.modeset = true;
                        data.rescan_controller = true;
                    }
                    Err(error) => {
                        warn!(%error, "DRM resume failed");
                        stop.stop();
                    }
                },
            }
        })?;
    Ok(())
}

/// The DRM output at its mode and `TEHUTI_SCALE`, advertised to clients.
fn drm_output(display: &DisplayHandle, mode: smithay::reexports::drm::control::Mode) -> Output {
    let output = Output::new(
        "tehuti-drm".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Tehuti".into(),
            model: "DRM".into(),
        },
    );
    let _global = output.create_global::<Compositor>(display);
    output.change_current_state(
        Some(mode.into()),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    tehuti::scale::apply(
        &output,
        mode.into(),
        tehuti::scale::from_env(i32::from(mode.size().0)),
    );
    output.set_preferred(mode.into());
    output
}

fn create_output(
    mut session: LibSeatSession,
    path: &std::path::Path,
    display: &DisplayHandle,
    compositor: &mut Compositor,
) -> Result<(Tty, smithay::backend::drm::DrmDeviceNotifier), Box<dyn Error>> {
    let device_fd = DrmDeviceFd::new(DeviceFd::from(
        session.open(path, rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC)?,
    ));
    let (mut device, drm_notifier) = DrmDevice::new(device_fd.clone(), false)?;
    let surface = connected_surface(&mut device)?;
    let mode = surface.pending_mode();
    let mut renderer = render::create(&device_fd, &surface)?;
    render::advertise(&mut renderer, display, compositor)?;
    let output = drm_output(display, mode);
    compositor.space.map_output(&output, (0, 0));
    compositor.output = Some(output.clone());
    compositor.fill_output();
    let libinput = create_libinput(&session)?;
    let tty = Tty {
        keyboard: tehuti::backend::EscapeKeyboard::default(),
        session,
        libinput,
        surface,
        device,
        renderer,
        damage: OutputDamageTracker::from_output(&output),
        output,
        next: false,
        pending: false,
        modeset: true,
    };
    Ok((tty, drm_notifier))
}

fn create_libinput(session: &LibSeatSession) -> Result<smithay::reexports::input::Libinput, Box<dyn Error>> {
    let mut libinput = smithay::reexports::input::Libinput::new_with_udev(
        smithay::backend::libinput::LibinputSessionInterface::from(session.clone()),
    );
    libinput
        .udev_assign_seat(&session.seat())
        .map_err(|()| "libinput seat assignment failed")?;
    Ok(libinput)
}

fn connected_surface(device: &mut DrmDevice) -> Result<DrmSurface, Box<dyn Error>> {
    let resources = device.resource_handles()?;
    let mut selected = None;
    for handle in resources.connectors() {
        let connector = device.get_connector(*handle, true)?;
        if connector.state() != connector::State::Connected {
            continue;
        }
        let Some(mode) = connector.modes().first().copied() else {
            continue;
        };
        for encoder in connector.encoders() {
            let encoder = device.get_encoder(*encoder)?;
            if let Some(crtc) = resources.filter_crtcs(encoder.possible_crtcs()).first().copied() {
                selected = Some((crtc, mode, *handle));
                break;
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let (crtc, mode, connector) = selected.ok_or("no connected DRM output with a usable mode")?;
    Ok(device.create_surface(crtc, mode, &[connector])?)
}

impl Tty {
    fn render(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        let (framebuffer, notice_drawn) = render::draw(
            &mut self.renderer,
            self.next,
            &self.output,
            &mut self.damage,
            &data.compositor,
        )?;
        submit(&self.surface, framebuffer, &mut self.modeset)?;
        self.pending = true;
        self.next = !self.next;
        if notice_drawn && data.compositor.secret_confirmation_text().is_some() {
            data.compositor.note_secret_confirmation_frame_submitted();
        }
        for window in data.compositor.space.elements() {
            window.send_frame(
                &self.output,
                data.compositor.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(self.output.clone()),
            );
        }
        data.compositor.space.refresh();
        data.display.flush_clients()?;
        Ok(())
    }
}

fn submit(
    surface: &DrmSurface,
    framebuffer: framebuffer::Handle,
    modeset: &mut bool,
) -> Result<(), Box<dyn Error>> {
    let plane = plane(surface, framebuffer);
    if *modeset {
        surface.commit([plane], true)?;
        *modeset = false;
    } else {
        surface.page_flip([plane], true)?;
    }
    Ok(())
}

fn plane(surface: &DrmSurface, framebuffer: framebuffer::Handle) -> PlaneState<'_> {
    let mode = surface.pending_mode();
    let (width, height) = mode.size();
    PlaneState {
        handle: surface.plane(),
        config: Some(PlaneConfig {
            src: Rectangle::from_size((f64::from(width), f64::from(height)).into()),
            dst: Rectangle::from_size((i32::from(width), i32::from(height)).into()),
            transform: Transform::Normal,
            alpha: 1.0,
            damage_clips: None,
            fb: framebuffer,
            fence: None,
        }),
    }
}
