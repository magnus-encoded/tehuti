//! A continuous value gauge above the bar.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    utils::Transform,
};
use tehuti_core::adjuster::Gauge;

const WIDTH: i32 = 400;
const HEIGHT: i32 = 104;

#[derive(Debug, Default)]
pub struct Buffer {
    cached: Option<(Gauge, MemoryRenderBuffer)>,
}

impl Buffer {
    /// # Errors
    /// The renderer could not import the gauge buffer.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        gauge: Option<&Gauge>,
    ) -> Result<Option<MemoryRenderBufferRenderElement<R>>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(gauge) = gauge else {
            self.cached = None;
            return Ok(None);
        };
        let Some(mode) = output.current_mode() else {
            return Ok(None);
        };
        if self.cached.as_ref().is_none_or(|(shown, _)| shown != gauge) {
            self.cached = raster(gauge).map(|pixels| {
                (
                    gauge.clone(),
                    MemoryRenderBuffer::from_slice(
                        &pixels,
                        Fourcc::Argb8888,
                        (WIDTH, HEIGHT),
                        1,
                        Transform::Normal,
                        None,
                    ),
                )
            });
        }
        let Some((_, buffer)) = &self.cached else {
            return Ok(None);
        };
        let size = crate::scale::logical_size(mode.size, output.current_scale().integer_scale());
        let top = size
            .h
            .saturating_sub(tehuti_core::bar::HEIGHT)
            .saturating_sub(HEIGHT)
            .saturating_sub(12);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (16.0, f64::from(top)),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .map(Some)
    }
}

#[must_use]
pub fn raster(gauge: &Gauge) -> Option<Vec<u8>> {
    let mut canvas = crate::settings::canvas::Canvas::new((WIDTH, HEIGHT))?;
    canvas.reference(400, 104);
    canvas.rectangle(0, 0, 400, 104, [32, 32, 32, 255]);
    canvas.text(
        16,
        10,
        &format!("{}  {}", gauge.name(), gauge.value()),
        2,
        [240, 236, 229, 255],
    );
    canvas.rectangle(16, 44, 368, 12, [80, 80, 80, 255]);
    let range = i64::from(gauge.maximum()).saturating_sub(i64::from(gauge.minimum()));
    let fill = i64::from(gauge.value())
        .saturating_sub(i64::from(gauge.minimum()))
        .saturating_mul(368)
        .checked_div(range)
        .unwrap_or_default();
    canvas.rectangle(
        16,
        44,
        usize::try_from(fill).unwrap_or_default(),
        12,
        [119, 167, 88, 255],
    );
    canvas.text(
        16,
        72,
        "STICK: ADJUST  SOUTH: KEEP  EAST: REVERT",
        1,
        [240, 236, 229, 255],
    );
    Some(canvas.pixels)
}
