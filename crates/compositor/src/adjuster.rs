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

const WIDTH: i32 = 280;
const HEIGHT: i32 = 224;
const TRACK_X: usize = 16;
const TRACK_Y: usize = 40;
const TRACK_W: usize = 28;
const TRACK_H: usize = 130;

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
    canvas.reference(280, 224);
    canvas.rectangle(0, 0, 280, 224, [32, 32, 32, 255]);
    canvas.text(
        16,
        10,
        &format!("{}  {}", gauge.name(), gauge.value()),
        2,
        [240, 236, 229, 255],
    );
    canvas.rectangle(TRACK_X, TRACK_Y, TRACK_W, TRACK_H, [80, 80, 80, 255]);
    let range = i64::from(gauge.maximum()).saturating_sub(i64::from(gauge.minimum()));
    let fill = i64::from(gauge.value())
        .saturating_sub(i64::from(gauge.minimum()))
        .saturating_mul(i64::try_from(TRACK_H).unwrap_or_default())
        .checked_div(range)
        .unwrap_or_default();
    let fill = usize::try_from(fill).unwrap_or_default().min(TRACK_H);
    // The fill rises from the bottom of the track, matching the vertical stick.
    canvas.rectangle(
        TRACK_X,
        TRACK_Y.saturating_add(TRACK_H).saturating_sub(fill),
        TRACK_W,
        fill,
        [119, 167, 88, 255],
    );
    canvas.text(16, 184, "STICK: ADJUST", 1, [240, 236, 229, 255]);
    canvas.text(16, 198, "SOUTH: KEEP  EAST: REVERT", 1, [240, 236, 229, 255]);
    Some(canvas.pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pixels: &[u8], column: usize, row: usize) -> Vec<u8> {
        let offset = row
            .saturating_mul(usize::try_from(WIDTH).unwrap_or_default())
            .saturating_add(column)
            .saturating_mul(4);
        pixels.iter().skip(offset).take(4).copied().collect()
    }

    #[test]
    fn the_fill_rises_from_the_bottom_of_a_vertical_track() {
        let column = TRACK_X.saturating_add(TRACK_W.saturating_div(2));
        let low = raster(&Gauge::standalone("volume".into(), 0, 100, 25)).unwrap_or_default();
        let maxed = raster(&Gauge::standalone("volume".into(), 0, 100, 100)).unwrap_or_default();
        let track = pixel(&low, column, TRACK_Y.saturating_add(2));
        let fill = pixel(&maxed, column, TRACK_Y.saturating_add(2));
        assert_ne!(track, fill, "a full gauge fills the top of the track");
        assert_eq!(
            pixel(&low, column, TRACK_Y.saturating_add(TRACK_H).saturating_sub(2)),
            fill
        );
        assert_eq!(pixel(&low, column, TRACK_Y.saturating_add(2)), track);
        assert_eq!(
            pixel(
                &low,
                column.saturating_add(TRACK_W),
                TRACK_Y.saturating_add(TRACK_H).saturating_sub(2)
            ),
            pixel(&low, 1, 1)
        );
    }
}
