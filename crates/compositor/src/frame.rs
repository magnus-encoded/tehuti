//! The frame sequence both backends share: overlays and notices, render, then, after the
//! backend has submitted the frame, the confirmation note, frame callbacks, refresh and flush.
//! A backend only binds a target and submits it.

use core::{error::Error, time::Duration};

use smithay::{
    backend::renderer::{ImportAll, ImportMem, Renderer, damage::OutputDamageTracker, sync::SyncPoint},
    output::Output,
    reexports::wayland_server::Display,
};

use crate::{overlays::Overlays, state::Compositor};

/// What one output keeps between frames.
pub struct Surface<'output> {
    pub output: &'output Output,
    pub overlays: &'output mut Overlays,
    pub damage: &'output mut OutputDamageTracker,
    /// How many frames old the target buffer is; 0 redraws everything.
    pub age: usize,
}

/// A rendered frame, ready for the backend to submit.
pub struct Rendered {
    /// Whether the output changed; an unchanged frame is not submitted.
    pub drew: bool,
    /// Whether a notice is among the drawn overlays.
    pub notice_included: bool,
    /// Signals when the GPU has finished drawing.
    pub sync: SyncPoint,
}

impl Compositor {
    /// Draws the overlays, notices and windows of one output into `target`.
    ///
    /// # Errors
    /// Returns an error when an overlay or the output cannot be rendered.
    pub fn frame<R>(
        &self,
        renderer: &mut R,
        target: &mut R::Framebuffer<'_>,
        surface: &mut Surface<'_>,
    ) -> Result<Rendered, Box<dyn Error>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
        R::Error: 'static,
    {
        let clock = chrono::Local::now().format("%H:%M").to_string();
        let overlays = surface
            .overlays
            .elements(renderer, surface.output, self, &clock)?;
        let result = crate::wallpaper::render_output_at_age(
            surface.output,
            renderer,
            target,
            self,
            &overlays.elements,
            surface.damage,
            surface.age,
        )?;
        Ok(Rendered {
            drew: result.damage.is_some(),
            notice_included: overlays.notice_included,
            sync: result.sync,
        })
    }

    /// Finishes a frame the backend has submitted, or found unchanged.
    ///
    /// A submitted frame that showed a notice (`notice_submitted`) counts as the delivery of a
    /// pending secret confirmation.
    /// `callbacks` says whether windows get their frame callback now.
    ///
    /// # Errors
    /// Returns an error when flushing the clients fails.
    pub fn frame_done(
        &mut self,
        output: &Output,
        display: &mut Display<Self>,
        notice_submitted: bool,
        callbacks: bool,
    ) -> Result<(), std::io::Error> {
        if notice_submitted && self.secret_confirmation_text().is_some() {
            self.note_secret_confirmation_frame_submitted();
        }
        if callbacks {
            let elapsed = self.start_time.elapsed();
            for window in self.space.elements() {
                window.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
        }
        self.space.refresh();
        display.flush_clients()
    }
}
