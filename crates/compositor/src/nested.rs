//! The winit backend: a window in the host desktop that Tehuti renders its output into.
//! Host keyboard input on this window goes to the focused client; host pointer input is ignored.

use core::{cell::RefCell, error::Error};
use std::rc::Rc;

use smithay::{
    backend::{
        renderer::{ImportDma, damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, LoopSignal},
        wayland_server::DisplayHandle,
    },
    utils::{Rectangle, Transform},
};
use tracing::warn;

use tehuti::state::Compositor;

use crate::LoopData;

const REFRESH_MILLIHERTZ: i32 = 60_000;

struct Nested {
    backend: Rc<RefCell<WinitGraphicsBackend<GlesRenderer>>>,
    output: Output,
    damage_tracker: OutputDamageTracker,
    stop: LoopSignal,
    overlays: tehuti::overlays::Overlays,
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "conflicts with the unreachable_pub rustc lint the workspace enables"
)]
pub(crate) fn insert(
    event_loop: &EventLoop<LoopData>,
    display: &DisplayHandle,
    compositor: &mut Compositor,
) -> Result<(), Box<dyn Error>> {
    let (mut backend, winit_source) = winit::init::<GlesRenderer>()?;
    resize_test_window(&backend)?;
    let mode = Mode {
        size: backend.window_size(),
        refresh: REFRESH_MILLIHERTZ,
    };
    let (formats, render_device) = tehuti::rendering::client_formats(backend.renderer(), display);
    let backend = Rc::new(RefCell::new(backend));
    compositor.set_capture_source(&backend, |backend, compositor, clock| {
        tehuti::rendering::capture_frame(backend.renderer(), compositor, clock)
    });
    let importer = backend.clone();
    compositor.enable_dmabuf(
        display,
        formats.iter().copied(),
        render_device,
        move |dmabuf| match importer.borrow_mut().renderer().import_dmabuf(dmabuf, None) {
            Ok(_) => true,
            Err(error) => {
                warn!(%error, "nested client dmabuf import failed");
                false
            }
        },
    )?;
    let output = Output::new(
        "tehuti".to_owned(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Tehuti".into(),
            model: "Nested".into(),
        },
    );
    let _global = output.create_global::<Compositor>(display);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    tehuti::scale::apply(&output, mode, tehuti::scale::from_env(mode.size.w));
    output.set_preferred(mode);
    compositor.space.map_output(&output, (0, 0));
    compositor.output = Some(output.clone());
    compositor.fill_output();

    let (ping, source) = smithay::reexports::calloop::ping::make_ping()?;
    let redraw_backend = backend.clone();
    let _wallpaper_token = event_loop.handle().insert_source(source, move |(), (), _| {
        redraw_backend.borrow().window().request_redraw();
    })?;
    tehuti::wallpaper::set_redraw_ping(ping)?;

    let mut nested = Nested {
        backend,
        damage_tracker: OutputDamageTracker::from_output(&output),
        output,
        stop: event_loop.get_signal(),
        overlays: tehuti::overlays::Overlays::default(),
    };
    let _winit_token = event_loop
        .handle()
        .insert_source(winit_source, move |event, (), data| nested.handle(&event, data))?;
    Ok(())
}

fn resize_test_window(backend: &WinitGraphicsBackend<GlesRenderer>) -> Result<(), Box<dyn Error>> {
    if std::env::var_os("TEHUTI_TEST_SIZE").is_some() {
        let (width, height) = tehuti::driver::test_size()?;
        let _size = backend
            .window()
            .request_inner_size(smithay::reexports::winit::dpi::PhysicalSize::new(
                u32::try_from(width)?,
                u32::try_from(height)?,
            ));
    }
    Ok(())
}

impl Nested {
    fn handle(&mut self, event: &WinitEvent, data: &mut LoopData) {
        match event {
            WinitEvent::Resized { size, .. } => {
                let mode = Mode {
                    size: *size,
                    refresh: REFRESH_MILLIHERTZ,
                };
                let scale = self.output.current_scale().integer_scale();
                tehuti::scale::apply(&self.output, mode, scale);
                data.compositor.fill_output();
            }
            WinitEvent::Redraw => self.redraw(data),
            WinitEvent::CloseRequested => self.stop.stop(),
            WinitEvent::Input(smithay::backend::input::InputEvent::Keyboard { event }) => {
                use smithay::backend::input::KeyboardKeyEvent as _;
                data.compositor.host_key(
                    event.key_code(),
                    event.state() == smithay::backend::input::KeyState::Pressed,
                );
            }
            WinitEvent::Focus(false) => data.compositor.release_host_keys(),
            WinitEvent::Input(_) | WinitEvent::Focus(true) => {}
        }
    }

    fn redraw(&mut self, data: &mut LoopData) {
        let size = self.backend.borrow().window_size();
        let notice_drawn = self.render_frame(data, size);
        if let Err(error) = data
            .compositor
            .frame_done(&self.output, &mut data.display, notice_drawn, true)
        {
            warn!(%error, "flush failed");
        }
        self.backend.borrow().window().request_redraw();
    }

    /// Whether the submitted frame drew a notice.
    fn render_frame(
        &mut self,
        data: &LoopData,
        size: smithay::utils::Size<i32, smithay::utils::Physical>,
    ) -> bool {
        let notice_drawn = match self.backend.borrow_mut().bind() {
            Ok((renderer, mut framebuffer)) => {
                let mut surface = tehuti::frame::Surface {
                    output: &self.output,
                    overlays: &mut self.overlays,
                    damage: &mut self.damage_tracker,
                    age: 0,
                };
                match data.compositor.frame(renderer, &mut framebuffer, &mut surface) {
                    Ok(frame) => frame.drew && frame.notice_included,
                    Err(error) => {
                        warn!(?error, "render failed");
                        false
                    }
                }
            }
            Err(error) => {
                warn!(?error, "bind failed");
                false
            }
        };
        match self
            .backend
            .borrow_mut()
            .submit(Some(&[Rectangle::from_size(size)]))
        {
            Ok(()) => notice_drawn,
            Err(error) => {
                warn!(?error, "submit failed");
                false
            }
        }
    }
}
