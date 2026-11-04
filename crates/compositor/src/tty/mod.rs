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
        udev::{UdevBackend, primary_gpu},
    },
    output::{Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, RegistrationToken},
        drm::control::{Device, Mode, ModeTypeFlags, connector, framebuffer},
        wayland_server::{DisplayHandle, backend::GlobalId},
    },
    utils::{DeviceFd, Rectangle, Transform},
};
use std::rc::Rc;
use tehuti::state::Compositor;
use tracing::{info, trace, warn};

struct Tty {
    keyboard: tehuti::backend::EscapeKeyboard,
    libinput: smithay::reexports::input::Libinput,
    scanout: Option<Scanout>,
    device: DrmDevice,
    rescan_pending: bool,
    // Weak handle; the event loop's notifier owns the seat during teardown.
    session: LibSeatSession,
}

struct Scanout {
    renderer: render::DrmRendering,
    // Rollback wins on a failed frame and clears on restore or successful scanout;
    // preview retains the confirmed mode until confirmation or reversion.
    preview: Option<(Mode, render::DrmRendering, bool)>,
    rollback: Option<(Mode, render::DrmRendering, bool)>,
    surface: DrmSurface,
    confirmed_mode: Mode,
    remaining_modes: std::collections::VecDeque<Mode>,
    listed_modes: Vec<Mode>,
    output: Output,
    global: GlobalId,
    damage: OutputDamageTracker,
    next: bool,
    pending: bool,
    modeset: bool,
    recovery: tehuti::frame_recovery::Recovery,
    /// Frames drawn since start, which dates each scanout buffer's contents.
    drawn: u64,
    idle_frame_callbacks: tehuti::pacing::Interval,
    statistics: FrameStatistics,
}

/// Frame counters, logged at debug level.
struct FrameStatistics {
    drawn: u32,
    unchanged: u32,
    report: tehuti::pacing::Interval,
}

impl FrameStatistics {
    fn count(&mut self, drawn: bool) {
        if drawn {
            self.drawn = self.drawn.saturating_add(1);
        } else {
            self.unchanged = self.unchanged.saturating_add(1);
        }
        if self.report.due(std::time::Instant::now()) {
            tracing::debug!(
                drawn = self.drawn,
                unchanged = self.unchanged,
                period = ?tehuti::pacing::STATISTICS_PERIOD,
                "DRM frames"
            );
            self.drawn = 0;
            self.unchanged = 0;
        }
    }
}

fn insert_device_watch(
    event_loop: &EventLoop<LoopData>,
    udev: UdevBackend,
    tty: &Rc<RefCell<Tty>>,
) -> Result<RegistrationToken, Box<dyn Error>> {
    let device_stop = event_loop.get_signal();
    let device_id = tty.borrow().device.device_id();
    let device_tty = tty.clone();
    let udev_token = event_loop.handle().insert_source(udev, move |event, (), _| {
        if tehuti::drm_hotplug::handle_event(&event, device_id, || {
            device_tty.borrow_mut().rescan_pending = true;
        })
        .is_break()
        {
            warn!("DRM device removed; ending session");
            device_stop.stop();
        }
    })?;
    Ok(udev_token)
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "conflicts with the workspace unreachable_pub lint"
)]
pub(crate) fn insert(
    event_loop: &EventLoop<'static, LoopData>,
    display: &DisplayHandle,
    compositor: &mut Compositor,
) -> Result<crate::Redraw, Box<dyn Error>> {
    let (session, notifier) = LibSeatSession::new()?;
    let udev = UdevBackend::new(session.seat())?;
    let path = primary_gpu(session.seat())?
        .or_else(|| udev.device_list().next().map(|(_, path)| path.to_owned()))
        .ok_or("no DRM device on seat")?;
    let (initialized_tty, drm_notifier) = create_output(session, &path, display, compositor)?;
    let tty = Rc::new(RefCell::new(initialized_tty));
    let seat_token = insert_seat(event_loop, notifier, &tty)?;
    let keyboard_token = insert_keyboard(event_loop, &tty)?;
    let frames_handle = Rc::new(event_loop.handle());
    let frames = tehuti::frame_recovery::Driver::new(&frames_handle, &tty, event_loop.get_signal());
    let flip_tty = tty.clone();
    let flip_frames = frames.clone();
    let flip_stop = event_loop.get_signal();
    let drm_token = event_loop
        .handle()
        .insert_source(drm_notifier, move |event, _, data| match event {
            DrmEvent::VBlank(metadata) => {
                trace!(?metadata, "DRM vblank");
                if let Some(scanout) = &mut flip_tty.borrow_mut().scanout {
                    scanout.pending = false;
                }
                flip_frames.render(data, true);
            }
            DrmEvent::Error(error) => {
                warn!(%error, "DRM event failed");
                flip_stop.stop();
            }
        })?;
    let udev_token = insert_device_watch(event_loop, udev, &tty)?;
    let wallpaper_token = insert_wallpaper_redraw(event_loop, frames.clone())?;
    let shutdown_tty = tty;
    let draw = Box::new(move |data: &mut LoopData| {
        // The redraw owns the handle that retry timers reach through.
        let _owner = &frames_handle;
        frames.render(data, false);
    });
    Ok(crate::Redraw {
        draw,
        shutdown: Box::new(move |loop_to_stop, seat_active| {
            let handle = loop_to_stop.handle();
            if (!seat_active || !shutdown_tty.borrow().session.is_active())
                && shutdown_tty.borrow().device.is_active()
            {
                shutdown_tty.borrow_mut().device.pause();
            }
            handle.remove(keyboard_token);
            handle.remove(drm_token);
            handle.remove(wallpaper_token);
            handle.remove(udev_token);
            // The DRM device, EGL renderer and libinput context are now gone.
            // The notifier is the strong owner of the libseat seat.
            drop(shutdown_tty);
            handle.remove(seat_token);
        }),
    })
}

type Frames = tehuti::frame_recovery::Driver<'static, LoopData, Tty>;

fn insert_wallpaper_redraw(
    event_loop: &EventLoop<'static, LoopData>,
    frames: Frames,
) -> Result<RegistrationToken, Box<dyn Error>> {
    let (ping, source) = smithay::reexports::calloop::ping::make_ping()?;
    let wallpaper_token = event_loop
        .handle()
        .insert_source(source, move |(), (), data| frames.render(data, true))?;
    tehuti::wallpaper::set_redraw_ping(ping)?;
    Ok(wallpaper_token)
}

fn insert_keyboard(
    event_loop: &EventLoop<LoopData>,
    tty: &Rc<RefCell<Tty>>,
) -> Result<RegistrationToken, Box<dyn Error>> {
    let keyboard_tty = tty.clone();
    let keyboard_stop = event_loop.get_signal();
    let input_backend = smithay::backend::libinput::LibinputInputBackend::new(tty.borrow().libinput.clone());
    let input_token = event_loop
        .handle()
        .insert_source(input_backend, move |event, (), data| {
            use smithay::backend::input::{
                AbsolutePositionEvent, ButtonState, InputEvent, KeyState, KeyboardKeyEvent,
                PointerButtonEvent, PointerMotionEvent,
            };
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
                    None => {
                        data.compositor
                            .host_key(event.key_code(), event.state() == KeyState::Pressed);
                        data.pacing.input_arrived = true;
                    }
                }
            } else if let InputEvent::PointerMotion { event } = event {
                data.compositor
                    .host_pointer_motion(event.delta_x(), event.delta_y());
                data.pacing.input_arrived = true;
            } else if let InputEvent::PointerMotionAbsolute { event } = event {
                data.compositor
                    .host_pointer_absolute(event.x_transformed(1), event.y_transformed(1));
                data.pacing.input_arrived = true;
            } else if let InputEvent::PointerButton { event } = event {
                use tehuti_core::dispatch::PointerButton;
                let button = match event.button_code() {
                    0x110 => Some(PointerButton::Left),
                    0x111 => Some(PointerButton::Right),
                    _ => None,
                };
                if let Some(button) = button {
                    data.compositor
                        .host_pointer_button(button, event.state() == ButtonState::Pressed);
                    data.pacing.input_arrived = true;
                }
            } else if matches!(event, InputEvent::DeviceRemoved { .. }) {
                keyboard_tty.borrow_mut().keyboard = tehuti::backend::EscapeKeyboard::default();
                data.compositor.release_host_keys();
            }
        })?;
    Ok(input_token)
}

fn insert_seat(
    event_loop: &EventLoop<LoopData>,
    notifier: smithay::backend::session::libseat::LibSeatSessionNotifier,
    seat_tty: &Rc<RefCell<Tty>>,
) -> Result<RegistrationToken, Box<dyn Error>> {
    let seat_tty = Rc::downgrade(seat_tty);
    let weak_handle = event_loop.handle().downgrade();
    let stop = event_loop.get_signal();
    let seat_token = event_loop
        .handle()
        .insert_source(notifier, move |event, (), data| {
            let Some(seat_tty) = seat_tty.upgrade() else {
                return;
            };
            let mut tty = seat_tty.borrow_mut();
            match event {
                Event::PauseSession => {
                    data.seat_active = false;
                    data.compositor.release_host_keys();
                    data.controllers.clear();
                    data.compositor
                        .controller_input(tehuti_core::controller::Input::Disconnected);
                    for (_, token) in core::mem::take(&mut data.controller_tokens) {
                        if let Some(handle) = weak_handle.upgrade() {
                            handle.remove(token);
                        }
                    }
                    data.discovery = tehuti::controller::Discovery::Searching {
                        retry_at: Duration::ZERO,
                    };
                    tty.keyboard = tehuti::backend::EscapeKeyboard::default();
                    tty.libinput.suspend();
                    tty.device.pause();
                    if let Some(scanout) = &mut tty.scanout {
                        scanout.pending = false;
                    }
                }
                Event::ActivateSession => match tty.device.activate(true) {
                    Ok(()) => {
                        if let Err(error) = tty.libinput.resume() {
                            warn!(?error, "keyboard resume failed");
                            stop.stop();
                        }
                        data.seat_active = true;
                        data.pacing.input_arrived = true;
                        tty.rescan_pending = true;
                        data.rescan_controller = true;
                    }
                    Err(error) => {
                        warn!(%error, "DRM resume failed");
                        stop.stop();
                    }
                },
            }
        })?;
    Ok(seat_token)
}

/// The DRM output at its mode and `TEHUTI_SCALE`, advertised to clients.
fn drm_output(display: &DisplayHandle, mode: Mode) -> (Output, GlobalId) {
    let output = Output::new(
        "tehuti-drm".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Tehuti".into(),
            model: "DRM".into(),
        },
    );
    let global = output.create_global::<Compositor>(display);
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
    (output, global)
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
        mut renderer,
        remaining_modes,
        listed_modes,
    } = connected_surface(&mut device)?;
    advertise(&mut renderer, display, compositor);
    let mode = surface.pending_mode();

    let (output, global) = drm_output(display, mode);
    compositor.space.map_output(&output, (0, 0));
    compositor.output = Some(output.clone());
    compositor.configure_display_modes(
        listed_modes.iter().map(mode_label).collect(),
        listed_modes
            .iter()
            .position(|candidate| *candidate == mode)
            .unwrap_or_default(),
    );
    compositor.fill_output();
    let libinput = create_libinput(&session)?;
    let scanout = Scanout::new(surface, renderer, remaining_modes, listed_modes, output, global);
    let tty = Tty {
        keyboard: tehuti::backend::EscapeKeyboard::default(),
        libinput,
        scanout: Some(scanout),
        device,
        rescan_pending: false,
        session,
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
        let saved = saved_mode().as_deref().and_then(mode::Request::parse);
        let (indices, matched) = mode::ordered_with_saved(&listed, parsed, saved);
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

/// Routes client dmabuf imports to `renderer` as soon as it exists, so none reach a replaced one.
fn advertise(renderer: &mut render::DrmRendering, display: &DisplayHandle, compositor: &mut Compositor) {
    if let Err(error) = render::advertise(renderer, display, compositor) {
        warn!(%error, "DRM client format advertisement failed");
    }
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
    fn rescan(&mut self, data: &mut LoopData) {
        self.rescan_pending = false;
        if let Err(error) = self.device.reset_state() {
            warn!(%error, "DRM state reset failed during connector rescan");
        }
        if let Some(scanout) = self.scanout.take() {
            data.compositor.space.unmap_output(&scanout.output);
            data.display
                .handle()
                .remove_global::<Compositor>(scanout.global.clone());
            drop(scanout);
        }
        data.compositor.output = None;
        data.compositor.configure_display_modes(Vec::new(), 0);
        match connected_surface(&mut self.device) {
            Ok(ConnectedSurface {
                surface,
                mut renderer,
                remaining_modes,
                listed_modes,
            }) => {
                advertise(&mut renderer, &data.display.handle(), &mut data.compositor);
                let mode = surface.pending_mode();
                let (output, global) = drm_output(&data.display.handle(), mode);
                data.compositor.space.map_output(&output, (0, 0));
                data.compositor.output = Some(output.clone());
                data.compositor.configure_display_modes(
                    listed_modes.iter().map(mode_label).collect(),
                    listed_modes
                        .iter()
                        .position(|candidate| *candidate == mode)
                        .unwrap_or_default(),
                );
                data.compositor.fill_output();
                self.scanout = Some(Scanout::new(
                    surface,
                    renderer,
                    remaining_modes,
                    listed_modes,
                    output,
                    global,
                ));
            }
            Err(error) => {
                warn!(%error, "DRM connector rescan found no usable output; waiting for a device change");
            }
        }
    }
}

impl tehuti::frame_recovery::Frames<LoopData> for Tty {
    fn render(
        &mut self,
        data: &mut LoopData,
        tick: bool,
    ) -> Result<Option<std::time::Instant>, Box<dyn Error>> {
        if !data.seat_active || !self.device.is_active() {
            return Ok(None);
        }
        if self.rescan_pending {
            self.rescan(data);
        }
        match &mut self.scanout {
            Some(scanout) => scanout.render(&self.device, data, tick),
            None => Ok(None),
        }
    }
}

impl Scanout {
    fn new(
        surface: DrmSurface,
        renderer: render::DrmRendering,
        remaining_modes: std::collections::VecDeque<Mode>,
        listed_modes: Vec<Mode>,
        output: Output,
        global: GlobalId,
    ) -> Self {
        Self {
            confirmed_mode: surface.pending_mode(),
            surface,
            renderer,
            remaining_modes,
            rollback: None,
            listed_modes,
            preview: None,
            damage: OutputDamageTracker::from_output(&output),
            output,
            global,
            next: false,
            pending: false,
            modeset: true,
            recovery: tehuti::frame_recovery::Recovery::default(),
            drawn: 0,
            idle_frame_callbacks: tehuti::pacing::Interval::new(tehuti::pacing::IDLE_FRAME_CALLBACK),
            statistics: FrameStatistics {
                drawn: 0,
                unchanged: 0,
                report: tehuti::pacing::Interval::new(tehuti::pacing::STATISTICS_PERIOD),
            },
        }
    }

    fn settings_request(&mut self, device: &DrmDevice, data: &mut LoopData) {
        while let Some((index, persist)) =
            tehuti::frame_recovery::when_idle(self.pending, || data.compositor.take_settings_request())
        {
            let Some(mode) = self.listed_modes.get(index).copied() else {
                data.compositor.display_mode_applied(None);
                continue;
            };
            if persist {
                self.confirmed_mode = mode;
                self.preview = None;
                let result = tehuti::display_mode_store::Store::from_env()
                    .and_then(|store| store.write(&mode_label(&mode)));
                data.compositor.display_mode_saved(result);
                continue;
            }
            let confirmed = self.confirmed_mode;
            let result = tehuti::frame_recovery::switch_or_restore(
                &mut (&mut *self, &mut *data),
                confirmed,
                |(scanout, data)| scanout.switch_settings_mode(device, mode, index, data),
                |(scanout, data), confirmed| {
                    scanout.restore_mode(device, confirmed, data);
                    scanout.modeset = true;
                    data.compositor.display_mode_applied(None);
                },
            );
            if let Err(error) = result {
                warn!(%error, "display mode rejected; restoring confirmed output");
            }
            return;
        }
    }

    fn switch_settings_mode(
        &mut self,
        device: &DrmDevice,
        mode: Mode,
        index: usize,
        data: &mut LoopData,
    ) -> Result<(), Box<dyn Error>> {
        let previous = self.surface.current_mode();
        let previous_next = self.next;
        let reverting = self.preview.as_ref().is_some_and(|(saved, _, _)| *saved == mode);
        let attempt = self
            .surface
            .use_mode(mode)
            .map_err(Box::<dyn Error>::from)
            .and_then(|()| {
                if reverting {
                    mode::restored_renderer(mode, self.preview.take(), || {
                        render::create(device.device_fd(), &self.surface)
                    })
                } else {
                    render::create(device.device_fd(), &self.surface).map(|renderer| (renderer, false))
                }
            });
        let (renderer, next) = attempt?;
        let previous_renderer = core::mem::replace(&mut self.renderer, renderer);
        advertise(&mut self.renderer, &data.display.handle(), &mut data.compositor);
        self.rollback = Some((previous, previous_renderer, previous_next));
        self.next = next;
        self.set_output_mode(mode, data);
        self.modeset = true;
        // Keep the confirmed scanout alive until the preview is confirmed or reverted.
        let retained = self.rollback.take();
        match self.render_frame(data) {
            Ok(()) => {
                self.recovery.reset();
                if !reverting {
                    self.preview = retained;
                }
                data.compositor.display_mode_applied(Some(index));
            }
            Err(error) => {
                self.rollback = retained;
                return Err(error);
            }
        }
        Ok(())
    }

    fn set_output_mode(&mut self, mode: Mode, data: &mut LoopData) {
        self.output
            .change_current_state(Some(mode.into()), None, None, None);
        tehuti::scale::apply(
            &self.output,
            mode.into(),
            self.output.current_scale().integer_scale(),
        );
        self.output.set_preferred(mode.into());
        self.damage = OutputDamageTracker::from_output(&self.output);
        data.compositor.fill_output();
    }

    fn fallback_mode(&mut self, device: &DrmDevice, data: &mut LoopData) -> bool {
        let previous = self.surface.pending_mode();
        while let Some(mode) = self.remaining_modes.pop_front() {
            let attempt = self
                .surface
                .use_mode(mode)
                .map_err(Box::<dyn Error>::from)
                .and_then(|()| render::create(device.device_fd(), &self.surface));
            match attempt {
                Ok(renderer) => {
                    let renderer = core::mem::replace(&mut self.renderer, renderer);
                    advertise(&mut self.renderer, &data.display.handle(), &mut data.compositor);
                    if self.rollback.is_none() {
                        self.rollback = Some((previous, renderer, self.next));
                    }
                    self.set_output_mode(mode, data);
                    data.compositor.configure_display_modes(
                        self.listed_modes.iter().map(mode_label).collect(),
                        self.listed_modes
                            .iter()
                            .position(|candidate| *candidate == mode)
                            .unwrap_or_default(),
                    );
                    self.next = false;
                    return true;
                }
                Err(error) => warn!(?mode, %error, "DRM fallback mode initialization failed"),
            }
        }
        false
    }

    fn reject_preview(&mut self, data: &mut LoopData) -> Result<bool, Box<dyn Error>> {
        let Some((mode, _, _)) = self.preview.as_ref() else {
            return Ok(false);
        };
        let mode = *mode;
        self.surface.use_mode(mode)?;
        if let Some((_, renderer, next)) = self.preview.take() {
            let previous = self.surface.current_mode();
            let renderer = core::mem::replace(&mut self.renderer, renderer);
            self.rollback = Some((previous, renderer, self.next));
            advertise(&mut self.renderer, &data.display.handle(), &mut data.compositor);
            self.next = next;
        }
        self.set_output_mode(mode, data);
        self.modeset = true;
        data.compositor.display_mode_applied(None);
        Ok(true)
    }

    fn restore_mode(&mut self, device: &DrmDevice, mode: Mode, data: &mut LoopData) {
        let restored = self
            .surface
            .use_mode(mode)
            .map_err(Box::<dyn Error>::from)
            .and_then(|()| {
                let retained = if self.preview.as_ref().is_some_and(|(saved, _, _)| *saved == mode) {
                    self.preview.take()
                } else if self.rollback.as_ref().is_some_and(|(saved, _, _)| *saved == mode) {
                    self.rollback.take()
                } else {
                    None
                };
                mode::restored_renderer(mode, retained, || {
                    render::create(device.device_fd(), &self.surface)
                })
            });
        match restored {
            Ok((renderer, next)) => {
                let previous = self.surface.current_mode();
                let retired = core::mem::replace(&mut self.renderer, renderer);
                if self.rollback.is_none() {
                    self.rollback = Some((previous, retired, self.next));
                }
                advertise(&mut self.renderer, &data.display.handle(), &mut data.compositor);
                self.next = next;
                self.set_output_mode(mode, data);
            }
            Err(error) => {
                if !self.recovery.retrying() {
                    warn!(%error, "DRM mode restoration failed; retaining scanout buffers");
                }
            }
        }
    }

    /// Draws a frame if the output changed. `tick` first advances time, for callers
    /// the loop's periodic work has not just run for.
    fn render(
        &mut self,
        device: &DrmDevice,
        data: &mut LoopData,
        tick: bool,
    ) -> Result<Option<std::time::Instant>, Box<dyn Error>> {
        if self.recovery.waiting(std::time::Instant::now()) {
            return Ok(None);
        }
        if tick {
            crate::advance_time(data);
        }
        self.settings_request(device, data);
        if self.pending {
            return Ok(None);
        }
        if self.recovery.retrying() {
            // The failed frame consumed its damage; redraw it whole.
            self.damage = OutputDamageTracker::from_output(&self.output);
        }
        let mut mode_error = None;
        let result = mode::retry(
            &mut (&mut *self, &mut *data),
            |(tty, data)| tty.render_frame(data),
            |(tty, data), error| {
                tty.damage = OutputDamageTracker::from_output(&tty.output);
                if tty.recovery.fallback_attempted() || tty.reject_preview(data)? || !tty.modeset {
                    return Err(error);
                }
                mode_error = Some(error);
                Ok(tty.fallback_mode(device, data))
            },
            |(tty, data)| tty.restore_mode(device, tty.confirmed_mode, data),
        )
        .and_then(|frame| frame.ok_or_else(|| mode_error.unwrap_or_else(|| "no usable DRM mode".into())));
        let mut fall_back = false;
        let retry = self
            .recovery
            .finish(std::time::Instant::now(), result, || fall_back = true)?;
        if fall_back {
            self.restore_mode(device, self.confirmed_mode, data);
            self.modeset = true;
            data.compositor.display_mode_applied(None);
        }
        Ok(retry)
    }

    fn render_frame(&mut self, data: &mut LoopData) -> Result<(), Box<dyn Error>> {
        let started = std::time::Instant::now();
        let drawn = render::draw(
            &mut self.renderer,
            self.next,
            render::Pass {
                drawn: &mut self.drawn,
                full: self.modeset,
            },
            &self.output,
            &mut self.damage,
            &data.compositor,
        )?;
        self.statistics.count(drawn.is_some());
        let Some(drawn) = drawn else {
            trace!(elapsed = ?started.elapsed(), "DRM frame unchanged");
            if self.idle_frame_callbacks.due(started) {
                self.send_frame_callbacks(data);
            }
            data.compositor.space.refresh();
            data.display.flush_clients()?;
            return Ok(());
        };
        trace!(elapsed = ?started.elapsed(), "DRM frame drawn");
        let was_modeset = self.modeset;
        submit(&self.surface, drawn.framebuffer, &mut self.modeset)?;
        if was_modeset {
            self.remaining_modes.clear();
            if self.rollback.is_some() && self.preview.is_none() {
                self.confirmed_mode = self.surface.pending_mode();
            }
            self.rollback = None;
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
        if drawn.notice_included && data.compositor.secret_confirmation_text().is_some() {
            data.compositor.note_secret_confirmation_frame_submitted();
        }
        self.idle_frame_callbacks.restart(started);
        self.send_frame_callbacks(data);
        data.compositor.space.refresh();
        data.display.flush_clients()?;
        Ok(())
    }

    fn send_frame_callbacks(&self, data: &LoopData) {
        for window in data.compositor.space.elements() {
            window.send_frame(
                &self.output,
                data.compositor.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(self.output.clone()),
            );
        }
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
