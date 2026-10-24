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

#[derive(Debug, Default)]
pub struct Overlays {
    bar: crate::bar::Buffer,
    keyboard: crate::osk::Buffer,
    cursor: Option<MemoryRenderBuffer>,
    notice: Option<(String, MemoryRenderBuffer)>,
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
        let mut elements = Vec::new();
        if let Some(location) = compositor.cursor_on(output) {
            let cursor = self.cursor.get_or_insert_with(crate::notice::cursor_buffer);
            elements.push(MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location,
                cursor,
                None,
                None,
                None,
                Kind::Cursor,
            )?);
        }
        if let Some(element) = crate::settings::element(renderer, compositor, output)? {
            elements.push(element);
        }
        let notice = compositor.visible_notice_text();
        if notice.as_ref() != self.notice.as_ref().map(|(shown, _)| shown) {
            self.notice = notice.map(|text| {
                let buffer = crate::notice::notice_buffer(&text);
                (text, buffer)
            });
        }
        if let Some((_, buffer)) = &self.notice {
            elements.push(MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (16.0, 16.0),
                buffer,
                None,
                None,
                None,
                Kind::Unspecified,
            )?);
        }
        let notice_included = self.notice.is_some();
        if let Some(element) = self
            .bar
            .element(renderer, output, &compositor.voice_bar(), clock)?
        {
            elements.push(element);
        }
        if let Some(element) = self
            .keyboard
            .element(renderer, output, compositor.on_screen_keyboard())?
        {
            elements.push(element);
        }
        Ok(OverlayElements {
            elements,
            notice_included,
        })
    }
}
