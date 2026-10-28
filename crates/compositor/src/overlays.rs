//! The overlays drawn above client surfaces on a DRM output.
//!
//! Each overlay keeps one buffer until its contents change, so an unchanged
//! overlay keeps its element identity and the damage tracker finds nothing to redraw.

use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    output::Output,
};

use crate::state::Compositor;
use tehuti_core::overlay::{Overlay, STACK};

#[derive(Debug, Default)]
pub struct Overlays {
    bar: crate::bar::Buffer,
    keyboard: crate::osk::Buffer,
    cursor: Option<MemoryRenderBuffer>,
    notice: Option<(Overlay, String, MemoryRenderBuffer)>,
}

/// Overlay elements, topmost first.
#[derive(Debug)]
pub struct OverlayElements<R: Renderer> {
    pub elements: Vec<MemoryRenderBufferRenderElement<R>>,
    pub notice_included: bool,
}

impl Overlays {
    /// The cursor, settings view, notice, bar and on-screen keyboard for `output`.
    ///
    /// # Errors
    /// The renderer could not import an overlay buffer.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        compositor: &Compositor,
        clock: &str,
    ) -> Result<OverlayElements<R>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let notice = compositor.visible_notice();
        self.update_notice(notice);
        let mut elements = Vec::new();
        for layer in STACK {
            if let Some(element) = self.element(layer, renderer, output, compositor, clock)? {
                elements.push(element);
            }
        }
        let notice_included = self.notice.is_some();

        Ok(OverlayElements {
            elements,
            notice_included,
        })
    }

    fn update_notice(&mut self, notice: Option<(Overlay, String)>) {
        let wanted = notice.as_ref().map(|(layer, text)| (*layer, text));
        let shown = self.notice.as_ref().map(|(layer, text, _)| (*layer, text));
        if wanted != shown {
            self.notice = notice.map(|(layer, text)| {
                let buffer = crate::notice::notice_buffer(&text);
                (layer, text, buffer)
            });
        }
    }

    fn element<R>(
        &mut self,
        layer: Overlay,
        renderer: &mut R,
        output: &Output,
        compositor: &Compositor,
        clock: &str,
    ) -> Result<Option<MemoryRenderBufferRenderElement<R>>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if self.notice.as_ref().is_some_and(|(shown, _, _)| *shown == layer) {
            return self.notice_element(renderer);
        }
        match layer {
            Overlay::Cursor => self.cursor_element(renderer, output, compositor),
            Overlay::Settings => crate::settings::element(renderer, compositor, output),
            Overlay::Bar => self.bar.element(renderer, output, &compositor.voice_bar(), clock),
            Overlay::Keyboard => self
                .keyboard
                .element(renderer, output, compositor.on_screen_keyboard()),
            Overlay::Notice
            | Overlay::MouseNotice
            | Overlay::SystemMenu
            | Overlay::Secret
            | Overlay::SecretUnavailable
            | Overlay::Mouse
            | Overlay::Application => Ok(None),
        }
    }

    fn cursor_element<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        compositor: &Compositor,
    ) -> Result<Option<MemoryRenderBufferRenderElement<R>>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(location) = compositor.cursor_on(output) else {
            return Ok(None);
        };
        let cursor = self.cursor.get_or_insert_with(crate::notice::cursor_buffer);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
            cursor,
            None,
            None,
            None,
            Kind::Cursor,
        )
        .map(Some)
    }

    fn notice_element<R>(
        &self,
        renderer: &mut R,
    ) -> Result<Option<MemoryRenderBufferRenderElement<R>>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some((_, _, buffer)) = &self.notice else {
            return Ok(None);
        };
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (16.0, 16.0),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .map(Some)
    }
}
