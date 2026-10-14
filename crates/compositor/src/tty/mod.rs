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
    listed_modes: Vec<Mode>,
    preview: Option<(Mode, render::DrmRendering)>,
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
    let render_stop = event_loop.get_signal();
    let _timer_token = event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, (), data| {
            let mut backend = tty.borrow_mut();
            if data.seat_active
                && backend.device.is_active()
                && let Err(error) = backend.render(data)
            {
                warn!(%error, "DRM frame failed");
                render_stop.stop();
            }
            TimeoutAction::ToDuration(Duration::from_millis(16))
        })?;
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
    let ConnectedSurface {
        surface,
        renderer,
        remaining_modes,
        listed_modes,
    } = connected_surface(&mut device)?;
    let mode = surface.pending_mode();
    let current = listed_modes
        .iter()
        .position(|candidate| *candidate == mode)
        .unwrap_or_default();
    compositor.configure_display_modes(listed_modes.iter().map(mode_label).collect(), current);

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
        listed_modes,
        preview: None,
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

struct ConnectedSurface {
    surface: DrmSurface,
    renderer: render::DrmRendering,
    remaining_modes: std::collections::VecDeque<Mode>,
    listed_modes: Vec<Mode>,
}

fn connected_surface(device: &mut DrmDevice) -> Result<ConnectedSurface, Box<dyn Error>> {
    let request = std::env::var("TEHUTI_MODE").ok();
    let saved = saved_mode();
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
        let (indices, matched) =
            mode::ordered_with_saved(&listed, parsed, saved.as_deref().and_then(mode::Request::parse));
        if request.is_some() && !matched {
            warn!(
                ?request,
                "TEHUTI_MODE does not match a listed mode; using saved or connector defaults"
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
                        Ok((surface, renderer)) => {
                            return Ok(ConnectedSurface {
                                surface,
                                renderer,
                                remaining_modes: remaining,
                                listed_modes: connector.modes().to_vec(),
                            });
                        }
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

fn saved_mode() -> Option<String> {
    tehuti::display_mode_store::Store::from_env()
        .and_then(|store| store.read())
        .unwrap_or_else(|error| {
            warn!(%error, "saved display mode unavailable");
            None
        })
}

fn mode_label(mode: &Mode) -> String {
    format!("{}x{}@{}", mode.size().0, mode.size().1, mode.vrefresh())
}

impl Tty {
    fn settings_request(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        while let Some((index, persist)) = data.compositor.take_settings_request() {
            let Some(mode) = self.listed_modes.get(index).copied() else {
                data.compositor.display_mode_applied(None);
                continue;
            };
            if persist {
                self.preview = None;
                if let Err(error) = tehuti::display_mode_store::Store::from_env()
                    .and_then(|store| store.write(&mode_label(&mode)))
                {
                    warn!(%error, "display mode persistence failed");
                }
                continue;
            }
            self.switch_mode(mode, index, data)?;
            return Ok(());
        }
        Ok(())
    }

    fn switch_mode(&mut self, mode: Mode, index: usize, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        let previous = self.surface.pending_mode();
        let reverting = self
            .preview
            .as_ref()
            .is_some_and(|(confirmed, _)| *confirmed == mode);
        let attempt = self
            .surface
            .use_mode(mode)
            .map_err(Box::<dyn Error>::from)
            .and_then(|()| {
                if reverting {
                    self.preview
                        .take()
                        .map(|(_, renderer)| renderer)
                        .ok_or_else(|| "confirmed renderer unavailable".into())
                } else {
                    render::create(self.device.device_fd(), &self.surface)
                }
            });
        let renderer = match attempt {
            Ok(renderer) => renderer,
            Err(error) => {
                warn!(%error, "display mode rejected");
                self.surface.use_mode(previous)?;
                data.compositor.display_mode_applied(None);
                return if reverting { Err(error) } else { Ok(()) };
            }
        };
        let previous_renderer = core::mem::replace(&mut self.renderer, renderer);
        self.update_mode(mode, data);
        match self.render_frame(data) {
            Ok(()) => {
                if !reverting {
                    self.preview = Some((previous, previous_renderer));
                }
                data.compositor.display_mode_applied(Some(index));
            }
            Err(error) => {
                warn!(%error, "display modeset rejected; restoring previous mode");
                self.surface.use_mode(previous)?;
                self.renderer = previous_renderer;
                self.update_mode(previous, data);
                data.compositor.display_mode_applied(None);
                self.render_frame(data)?;
                if reverting {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn update_mode(&mut self, mode: Mode, data: &mut LoopData) {
        self.output
            .change_current_state(Some(mode.into()), None, None, None);
        tehuti::scale::apply(
            &self.output,
            mode.into(),
            tehuti::scale::from_env(i32::from(mode.size().0)),
        );
        self.damage = OutputDamageTracker::from_output(&self.output);
        self.next = false;
        self.modeset = true;
        data.compositor.fill_output();
    }

    fn fallback_mode(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        while let Some(mode) = self.remaining_modes.pop_front() {
            let attempt = self
                .surface
                .use_mode(mode)
                .map_err(Box::<dyn Error>::from)
                .and_then(|()| render::create(self.device.device_fd(), &self.surface));
            match attempt {
                Ok(renderer) => {
                    self.renderer = renderer;
                    self.output
                        .change_current_state(Some(mode.into()), None, None, None);
                    tehuti::scale::apply(
                        &self.output,
                        mode.into(),
                        tehuti::scale::from_env(i32::from(mode.size().0)),
                    );
                    self.output.set_preferred(mode.into());
                    self.damage = OutputDamageTracker::from_output(&self.output);
                    self.next = false;
                    data.compositor.fill_output();
                    let current = self
                        .listed_modes
                        .iter()
                        .position(|candidate| *candidate == mode)
                        .unwrap_or_default();
                    data.compositor
                        .configure_display_modes(self.listed_modes.iter().map(mode_label).collect(), current);
                    return Ok(());
                }
                Err(error) => warn!(?mode, %error, "DRM fallback mode initialization failed"),
            }
        }
        Err("all listed DRM modes failed".into())
    }

    fn render(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        crate::advance_time(data);
        self.settings_request(data)?;
        if self.pending {
            return Ok(());
        }
        mode::retry(
            &mut (self, data),
            |(tty, data)| tty.render_frame(data),
            |(tty, data), error| {
                if !tty.modeset {
                    return Err(error);
                }
                warn!(%error, mode = ?tty.surface.pending_mode(), "DRM modeset failed; trying next listed mode");
                tty.fallback_mode(data)
            },
        )
    }

    fn render_frame(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        let started = std::time::Instant::now();
        let screenshot = data.compositor.take_screenshot_request();
        let (framebuffer, notice_drawn, saved) = render::draw(
            &mut self.renderer,
            self.next,
            &self.output,
            &mut self.damage,
            &data.compositor,
            screenshot.as_deref(),
        )?;
        if let Some(result) = saved {
            data.compositor.screenshot_saved(&result);
        }
        trace!(elapsed = ?started.elapsed(), "DRM frame drawn");
        let was_modeset = self.modeset;
        submit(&self.surface, framebuffer, &mut self.modeset)?;
        if was_modeset {
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
