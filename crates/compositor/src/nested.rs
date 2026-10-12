//! The winit backend: a window in the host desktop that Tehuti renders its output into.
//! Host keyboard and pointer input on this window is ignored; only the controller drives Tehuti.

use core::{error::Error, time::Duration};

use smithay::{
    backend::{
        renderer::{
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
    backend: WinitGraphicsBackend<GlesRenderer>,
    output: Output,
    damage_tracker: OutputDamageTracker,
    stop: LoopSignal,
    notice: MemoryRenderBuffer,
    notice_text: String,
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
    let (backend, winit_source) = winit::init::<GlesRenderer>()?;
    let mode = Mode {
        size: backend.window_size(),
        refresh: REFRESH_MILLIHERTZ,
    };
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
    output.set_preferred(mode);
    compositor.space.map_output(&output, (0, 0));
    compositor.output = Some(output.clone());

    let mut nested = Nested {
        backend,
        damage_tracker: OutputDamageTracker::from_output(&output),
        output,
        stop: event_loop.get_signal(),
        notice: tehuti::notice::speech_notice_buffer(),
        notice_text: "NO SPEECH MAPPING".into(),
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
                self.output.change_current_state(Some(mode), None, None, None);
                data.compositor.fill_output();
            }
            WinitEvent::Redraw => self.redraw(data),
            WinitEvent::CloseRequested => self.stop.stop(),
            WinitEvent::Input(_) | WinitEvent::Focus(_) => {}
        }
    }

    fn redraw(&mut self, data: &mut LoopData) {
        let size = self.backend.window_size();
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
        self.backend.window().request_redraw();
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
        let mut output_drew = false;
        let mut confirmation_element_included = false;
        match self.backend.bind() {
            Ok((renderer, mut framebuffer)) => {
                let notices = if visible_notice.is_some() {
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
                    Ok(result) if result.damage.is_some() => output_drew = true,
                    Ok(_) => {}
                    Err(error) => warn!(?error, "render failed"),
                }
            }
            Err(error) => warn!(?error, "bind failed"),
        }
        let submit_succeeded = match self.backend.submit(Some(&[Rectangle::from_size(size)])) {
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
}
