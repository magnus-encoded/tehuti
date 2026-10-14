//! The winit backend: a window in the host desktop that Tehuti renders its output into.
//! Host keyboard and pointer input on this window is ignored; only the controller drives Tehuti.

use core::{cell::RefCell, error::Error, time::Duration};
use std::rc::Rc;

use smithay::{
    backend::{
        renderer::{
            ImportDma,
            damage::OutputDamageTracker,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    desktop::space::render_output,
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
const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

struct Nested {
    backend: Rc<RefCell<WinitGraphicsBackend<GlesRenderer>>>,
    output: Output,
    damage_tracker: OutputDamageTracker,
    stop: LoopSignal,
    notice: MemoryRenderBuffer,
    notice_text: String,
    bar: tehuti::bar::Buffer,
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
    let mode = Mode {
        size: backend.window_size(),
        refresh: REFRESH_MILLIHERTZ,
    };
    let (formats, render_device) = tehuti::rendering::client_formats(backend.renderer(), display);
    let backend = Rc::new(RefCell::new(backend));
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

    let mut nested = Nested {
        backend,
        damage_tracker: OutputDamageTracker::from_output(&output),
        output,
        stop: event_loop.get_signal(),
        notice: tehuti::notice::speech_notice_buffer(),
        notice_text: "NO SPEECH MAPPING".into(),
        bar: tehuti::bar::Buffer::default(),
    };
    let _winit_token = event_loop
        .handle()
        .insert_source(winit_source, move |event, (), data| nested.handle(&event, data))?;
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
            WinitEvent::Input(_) | WinitEvent::Focus(_) => {}
        }
    }

    fn redraw(&mut self, data: &mut LoopData) {
        let size = self.backend.borrow().window_size();
        self.render_frame(data, size);
        let elapsed = data.compositor.start_time.elapsed();
        let output = &self.output;
        for window in data.compositor.space.elements() {
            window.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| Some(output.clone()));
        }
        data.compositor.space.refresh();
        if let Err(error) = data.display.flush_clients() {
            warn!(%error, "flush failed");
        }
        self.backend.borrow().window().request_redraw();
    }

    fn render_frame(
        &mut self,
        data: &mut LoopData,
        size: smithay::utils::Size<i32, smithay::utils::Physical>,
    ) {
        let confirmation_pending = data.compositor.secret_confirmation_text().is_some();
        let visible_notice = data.compositor.visible_notice_text();
        if let Some(text) = &visible_notice
            && text != &self.notice_text
        {
            self.notice = tehuti::notice::notice_buffer(text);
            self.notice_text.clone_from(text);
        }
        let screenshot = data.compositor.take_screenshot_request();
        let mut screenshot_result = None;
        let (confirmation_element_included, output_drew) = self.draw_bound_output(
            data,
            visible_notice.is_some(),
            confirmation_pending,
            screenshot,
            &mut screenshot_result,
        );
        if let Some(result) = screenshot_result {
            data.compositor.screenshot_saved(&result);
        }
        let submit_succeeded = match self
            .backend
            .borrow_mut()
            .submit(Some(&[Rectangle::from_size(size)]))
        {
            Ok(()) => true,
            Err(error) => {
                warn!(?error, "submit failed");
                false
            }
        };
        if confirmation_element_included && output_drew && submit_succeeded {
            data.compositor.note_secret_confirmation_frame_submitted();
        }
    }

    fn draw_bound_output(
        &mut self,
        data: &LoopData,
        visible_notice: bool,
        confirmation_pending: bool,
        screenshot: Option<std::path::PathBuf>,
        screenshot_result: &mut Option<std::io::Result<()>>,
    ) -> (bool, bool) {
        let mut output_drew = false;
        let mut confirmation_element_included = false;
        match self.backend.borrow_mut().bind() {
            Ok((renderer, mut framebuffer)) => {
                let mut notices = if visible_notice {
                    match MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        (16.0, 16.0),
                        &self.notice,
                        None,
                        None,
                        None,
                        Kind::Unspecified,
                    ) {
                        Ok(element) => {
                            confirmation_element_included = confirmation_pending;
                            vec![element]
                        }
                        Err(error) => {
                            warn!(?error, "notice render failed");
                            Vec::new()
                        }
                    }
                } else {
                    Vec::new()
                };
                let clock = chrono::Local::now().format("%H:%M").to_string();
                match self
                    .bar
                    .element(renderer, &self.output, data.compositor.voice_bar(), &clock)
                {
                    Ok(Some(element)) => notices.push(element),
                    Ok(None) => {}
                    Err(error) => warn!(?error, "bar render failed"),
                }
                match render_output::<_, MemoryRenderBufferRenderElement<GlesRenderer>, _, _>(
                    &self.output,
                    renderer,
                    &mut framebuffer,
                    1.0,
                    0,
                    [&data.compositor.space],
                    &notices,
                    &mut self.damage_tracker,
                    BACKGROUND,
                ) {
                    Ok(result) => {
                        output_drew = result.damage.is_some();
                        if let Some(path) = screenshot {
                            let size = self
                                .output
                                .current_mode()
                                .map_or((0, 0), |mode| (mode.size.w, mode.size.h));
                            let saved = tehuti::system::capture(renderer, &framebuffer, size, &path);
                            *screenshot_result = Some(saved);
                        }
                    }
                    Err(error) => warn!(?error, "render failed"),
                }
            }
            Err(error) => warn!(?error, "bind failed"),
        }
        (confirmation_element_included, output_drew)
    }
}
