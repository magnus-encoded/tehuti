//! Shared raster bar for GLES and pixman output rendering.

use font8x8::UnicodeFonts;
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
use tehuti_core::bar::{Bar, Segment, Tone};

#[derive(Debug, Default)]
pub struct Buffer {
    key: Option<(Vec<Segment>, String, i32, i32, i32)>,
    buffer: Option<MemoryRenderBuffer>,
}

impl Buffer {
    /// Imports the cached bar for either output renderer.
    ///
    /// # Errors
    /// The renderer could not import the raster buffer.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        bar: &Bar,
        clock: &str,
    ) -> Result<Option<MemoryRenderBufferRenderElement<R>>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some((buffer, position)) = self.update(output, bar, clock) else {
            return Ok(None);
        };
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .map(Some)
    }

    pub fn update(
        &mut self,
        output: &Output,
        bar: &Bar,
        clock: &str,
    ) -> Option<(&MemoryRenderBuffer, (f64, f64))> {
        let mode = output.current_mode()?;
        let scale = output.current_scale().integer_scale().max(1);
        let height = tehuti_core::bar::HEIGHT.saturating_mul(scale).min(mode.size.h);
        let segments = bar.segments();
        let key = (segments, clock.to_owned(), mode.size.w, scale, height);
        if self.key.as_ref() != Some(&key) {
            self.buffer = raster(&key.0, &key.1, mode.size.w, height, scale);
            self.key = Some(key);
        }
        self.buffer
            .as_ref()
            .map(|buffer| (buffer, (0.0, f64::from(mode.size.h.saturating_sub(height)))))
    }
}

const fn colour(tone: Tone) -> [u8; 4] {
    match tone {
        Tone::Unfocused => [55, 43, 34, 255],
        Tone::Context => [88, 65, 47, 255],
        Tone::Live => [119, 88, 36, 255],
        Tone::Result => [77, 96, 42, 255],
        Tone::Error => [65, 53, 138, 255],
    }
}

fn raster(
    segments: &[Segment],
    clock: &str,
    width: i32,
    height: i32,
    scale: i32,
) -> Option<MemoryRenderBuffer> {
    let width_usize = usize::try_from(width).ok()?;
    let height_usize = usize::try_from(height).ok()?;
    let mut canvas = Canvas {
        width: width_usize,
        height: height_usize,
        pixels: vec![0; width_usize.checked_mul(height_usize)?.checked_mul(4)?],
    };
    for pixel in canvas.pixels.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[30, 24, 20, 255]);
    }
    let unit = usize::try_from(scale).ok()?;
    let clock_width = 104usize.saturating_mul(unit).min(width_usize);
    let clock_start = width_usize.saturating_sub(clock_width);
    let arrow = 12usize.saturating_mul(unit);
    let mut next_start = 0usize;
    let mut chips = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let wanted = segment
            .text
            .chars()
            .count()
            .saturating_mul(16)
            .saturating_add(36)
            .saturating_mul(unit);
        let available = clock_start.saturating_sub(arrow).saturating_sub(next_start);
        let wanted = wanted.min(
            available
                .checked_div(segments.len().saturating_sub(index))
                .unwrap_or_default(),
        );
        let end = next_start
            .saturating_add(wanted)
            .min(clock_start.saturating_sub(arrow));
        if end <= next_start {
            break;
        }
        chips.push((next_start, end, segment));
        next_start = end;
    }
    canvas.rectangle(clock_start, width_usize, colour(Tone::Context));
    canvas.text(
        clock,
        clock_start.saturating_add(16usize.saturating_mul(unit)),
        width_usize,
        unit,
    );
    for (start, end, segment) in chips.into_iter().rev() {
        canvas.chip(start, end, segment, arrow, clock_start, unit);
    }
    Some(MemoryRenderBuffer::from_slice(
        &canvas.pixels,
        Fourcc::Argb8888,
        (width, height),
        scale,
        Transform::Normal,
        None,
    ))
}

struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Canvas {
    fn chip(&mut self, start: usize, end: usize, segment: &Segment, arrow: usize, limit: usize, unit: usize) {
        let background = colour(segment.tone);
        self.rectangle(start, end, background);
        let half = self.height.div_ceil(2).max(1);
        for row in 0..self.height {
            let reach = arrow
                .saturating_mul(half.saturating_sub(row.abs_diff(self.height / 2)))
                .checked_div(half)
                .unwrap_or_default();
            for column in end..end.saturating_add(reach).min(limit) {
                self.pixel(column, row, background);
            }
        }
        self.text(
            &segment.text,
            start.saturating_add(18usize.saturating_mul(unit)),
            end.saturating_sub(4usize.saturating_mul(unit)),
            unit,
        );
    }

    fn pixel(&mut self, column: usize, row: usize, colour: [u8; 4]) {
        if column >= self.width || row >= self.height {
            return;
        }
        let offset = row
            .saturating_mul(self.width)
            .saturating_add(column)
            .saturating_mul(4);
        if let Some(pixel) = self.pixels.get_mut(offset..offset.saturating_add(4)) {
            pixel.copy_from_slice(&colour);
        }
    }

    fn rectangle(&mut self, start: usize, end: usize, colour: [u8; 4]) {
        for row in 0..self.height {
            for column in start..end {
                self.pixel(column, row, colour);
            }
        }
    }

    fn text(&mut self, text: &str, start: usize, end: usize, unit: usize) {
        let size = 2usize.saturating_mul(unit);
        let advance = 8usize.saturating_mul(size);
        let top = self.height.saturating_sub(8usize.saturating_mul(size)) / 2;
        let capacity = end.saturating_sub(start).checked_div(advance).unwrap_or_default();
        let truncated = text.chars().count() > capacity;
        for (index, mut character) in text.chars().enumerate() {
            if truncated && index.saturating_add(1) == capacity {
                character = '…';
            }
            let left = start.saturating_add(index.saturating_mul(advance));
            if left.saturating_add(advance) > end {
                break;
            }
            let glyph = if character == '…' {
                [0, 0, 0, 0, 0, 0, 42, 0]
            } else {
                font8x8::BASIC_FONTS
                    .get(character)
                    .or_else(|| font8x8::LATIN_FONTS.get(character))
                    .unwrap_or([255, 129, 129, 129, 129, 129, 129, 255])
            };
            for (row, bits) in glyph.into_iter().enumerate() {
                for column in 0..8usize {
                    if bits
                        & (1u8
                            .checked_shl(u32::try_from(column).unwrap_or_default())
                            .unwrap_or_default())
                        == 0
                    {
                        continue;
                    }
                    for vertical in 0..size {
                        for horizontal in 0..size {
                            self.pixel(
                                left.saturating_add(column.saturating_mul(size))
                                    .saturating_add(horizontal),
                                top.saturating_add(row.saturating_mul(size))
                                    .saturating_add(vertical),
                                [239, 235, 229, 255],
                            );
                        }
                    }
                }
            }
        }
    }
}
