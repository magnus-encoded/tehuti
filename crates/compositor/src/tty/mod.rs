//! A libseat-owned DRM output with GLES rendering and a pixman fallback.

mod mode;
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
        drm::control::{Device, Mode, ModeTypeFlags, connector, framebuffer},
        wayland_server::DisplayHandle,
    },
    utils::{DeviceFd, Rectangle, Transform},
};
use std::rc::Rc;
use tehuti::state::Compositor;
use tracing::{info, trace, warn};

struct Tty {
    keyboard: tehuti::backend::EscapeKeyboard,
    session: LibSeatSession,
    libinput: smithay::reexports::input::Libinput,
    surface: DrmSurface,
    device: DrmDevice,
    renderer: render::DrmRendering,
    remaining_modes: std::collections::VecDeque<Mode>,
    rollback: Option<(Mode, render::DrmRendering, bool)>,
    output: Output,
    damage: OutputDamageTracker,
    next: bool,
    pending: bool,
    modeset: bool,
    retry_at: Option<std::time::Instant>,
    retry_delay: Duration,
}

fn insert_device_watch(
    event_loop: &EventLoop<LoopData>,
    udev: UdevBackend,
    device_id: libc::dev_t,
) -> Result<(), Box<dyn Error>> {
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
    Ok(())
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
        .insert_source(drm_notifier, move |event, _, data| match event {
            DrmEvent::VBlank(metadata) => {
                trace!(?metadata, "DRM vblank");
                let mut backend = flip_tty.borrow_mut();
                backend.pending = false;
                if data.seat_active
                    && backend.device.is_active()
                    && let Err(error) = backend.render(data)
                {
                    warn!(%error, "DRM frame failed");
                    flip_stop.stop();
                }
            }
            DrmEvent::Error(error) => {
                warn!(%error, "DRM event failed");
                flip_stop.stop();
            }
        })?;
    insert_device_watch(event_loop, udev, tty.borrow().device.device_id())?;
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
    let (mut device, drm_notifier) = DrmDevice::new(device_fd, false)?;
    let (surface, renderer, remaining_modes) = connected_surface(&mut device)?;
    let mode = surface.pending_mode();

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
        remaining_modes,
        rollback: None,
        damage: OutputDamageTracker::from_output(&output),
        output,
        next: false,
        pending: false,
        modeset: true,
        retry_at: None,
        retry_delay: Duration::ZERO,
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

fn connected_surface(
    device: &mut DrmDevice,
) -> Result<(DrmSurface, render::DrmRendering, std::collections::VecDeque<Mode>), Box<dyn Error>> {
    let request = std::env::var("TEHUTI_MODE").ok();
    let parsed = request.as_deref().and_then(mode::Request::parse);
    let resources = device.resource_handles()?;
    for handle in resources.connectors() {
        let connector = device.get_connector(*handle, true)?;
        if connector.state() != connector::State::Connected {
            continue;
        }
        let listed: Vec<_> = connector
            .modes()
            .iter()
            .map(|mode| mode::ListedMode {
                size: mode.size(),
                refresh: mode.vrefresh(),
                preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
            })
            .collect();
        let (indices, matched) = mode::ordered(&listed, parsed);
        if request.is_some() && !matched {
            warn!(
                ?request,
                "TEHUTI_MODE does not match a listed mode; using connector defaults"
            );
        }
        let modes: Vec<_> = indices
            .into_iter()
            .filter_map(|index| connector.modes().get(index).copied())
            .collect();
        for encoder in connector.encoders() {
            let encoder = device.get_encoder(*encoder)?;
            for crtc in resources.filter_crtcs(encoder.possible_crtcs()) {
                let mut remaining: std::collections::VecDeque<_> = modes.iter().copied().collect();
                while let Some(mode) = remaining.pop_front() {
                    let attempt = device
                        .create_surface(crtc, mode, &[*handle])
                        .map_err(Box::<dyn Error>::from)
                        .and_then(|surface| {
                            render::create(device.device_fd(), &surface).map(|renderer| (surface, renderer))
                        });
                    match attempt {
                        Ok((surface, renderer)) => return Ok((surface, renderer, remaining)),
                        Err(error) => {
                            warn!(?mode, %error, "DRM mode initialization failed; trying next listed mode");
                        }
                    }
                }
            }
        }
    }
    Err("no connected DRM output with a usable mode".into())
}

impl Tty {
    fn set_output_mode(&mut self, mode: Mode, data: &mut LoopData) {
        self.output
            .change_current_state(Some(mode.into()), None, None, None);
        tehuti::scale::apply(
            &self.output,
            mode.into(),
            tehuti::scale::from_env(i32::from(mode.size().0)),
        );
        self.output.set_preferred(mode.into());
        self.damage = OutputDamageTracker::from_output(&self.output);
        data.compositor.fill_output();
    }

    fn fallback_mode(&mut self, data: &mut LoopData) -> bool {
        let previous = self.surface.pending_mode();
        while let Some(mode) = self.remaining_modes.pop_front() {
            let attempt = self
                .surface
                .use_mode(mode)
                .map_err(Box::<dyn Error>::from)
                .and_then(|()| render::create(self.device.device_fd(), &self.surface));
            match attempt {
                Ok(renderer) => {
                    let renderer = core::mem::replace(&mut self.renderer, renderer);
                    if self.rollback.is_none() {
                        self.rollback = Some((previous, renderer, self.next));
                    }
                    self.set_output_mode(mode, data);
                    self.next = false;
                    return true;
                }
                Err(error) => warn!(?mode, %error, "DRM fallback mode initialization failed"),
            }
        }
        false
    }

    fn restore_mode(&mut self, data: &mut LoopData) {
        let mode = self.surface.current_mode();
        let restored = self
            .surface
            .use_mode(mode)
            .map_err(Box::<dyn Error>::from)
            .and_then(|()| {
                mode::restored_renderer(mode, self.rollback.take(), || {
                    render::create(self.device.device_fd(), &self.surface)
                })
            });
        match restored {
            Ok((renderer, next)) => {
                self.renderer = renderer;
                self.next = next;
                self.set_output_mode(mode, data);
            }
            Err(error) => warn!(%error, "DRM mode restoration failed; retaining scanout buffers"),
        }
    }

    fn render(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        if self
            .retry_at
            .is_some_and(|deadline| std::time::Instant::now() < deadline)
        {
            return Ok(());
        }
        crate::advance_time(data);
        let result = mode::retry(
            &mut (&mut *self, data),
            |(tty, data)| tty.render_frame(data),
            |(tty, data), error| {
                if !tty.modeset {
                    return Err(error);
                }
                let fallback = tty.fallback_mode(data);
                if fallback {
                    warn!(%error, "DRM modeset failed; trying next listed mode");
                } else {
                    warn!(%error, delay = ?mode::next_backoff(tty.retry_delay), "all listed DRM modes failed; backing off before retrying restored output");
                }
                Ok(fallback)
            },
            |(tty, data)| tty.restore_mode(data),
        )?;
        if result.is_some() {
            self.retry_at = None;
            self.retry_delay = Duration::ZERO;
        } else {
            self.retry_delay = mode::next_backoff(self.retry_delay);
            self.retry_at = std::time::Instant::now().checked_add(self.retry_delay);
        }
        Ok(())
    }

    fn render_frame(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        let started = std::time::Instant::now();
        let (framebuffer, notice_drawn) = render::draw(
            &mut self.renderer,
            self.next,
            &self.output,
            &mut self.damage,
            &data.compositor,
        )?;
        trace!(elapsed = ?started.elapsed(), "DRM frame drawn");
        let was_modeset = self.modeset;
        submit(&self.surface, framebuffer, &mut self.modeset)?;
        if was_modeset {
            self.rollback = None;
            if let Err(error) =
                render::advertise(&mut self.renderer, &data.display.handle(), &mut data.compositor)
            {
                warn!(%error, "DRM client format advertisement failed");
            }
            let mode = self.surface.pending_mode();
            info!(
                width = mode.size().0,
                height = mode.size().1,
                refresh_hz = mode.vrefresh(),
                "DRM output mode; frames follow vblank"
            );
        }
        trace!(elapsed = ?started.elapsed(), "DRM frame committed");
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
