//! Shared raster bar for GLES and pixman output rendering.

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
use tehuti_core::{
    bar::{Bar, Segment, Tone},
    ticker,
};

/// What the bar shows; the buffer is redrawn only when this changes.
type Key = (
    Vec<Segment>,
    String,
    i32,
    i32,
    i32,
    Option<(std::sync::Arc<str>, usize)>,
);

/// The space between ticker entries.
const TICKER_SEPARATOR: &str = "   \u{b7}   ";

#[derive(Debug, Default)]
pub struct Buffer {
    key: Option<Key>,
    image: Option<MemoryRenderBuffer>,
    ticker_text: Option<(std::sync::Arc<[ticker::Entry]>, std::sync::Arc<str>)>,
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
        let unit = usize::try_from(scale).ok()?;
        let width = usize::try_from(mode.size.w).ok()?;
        let layout = Layout::new(&segments, width, unit);
        let ticker = bar.ticker().map(|view| {
            let text = self.ticker_text(&view.entries);
            let content = text.chars().count().saturating_mul(8).saturating_mul(unit);
            let offset = ticker::scroll_offset(
                view.step,
                content,
                layout.ticker.1.saturating_sub(layout.ticker.0),
                ticker::STEP_PIXELS.saturating_mul(unit),
                ticker::REPEAT_GAP.saturating_mul(unit),
            );
            (text, offset)
        });
        let key = (segments, clock.to_owned(), mode.size.w, scale, height, ticker);
        if self.key.as_ref() != Some(&key) {
            self.image = raster(&key, &layout, height);
            self.key = Some(key);
        }
        self.image
            .as_ref()
            .map(|buffer| (buffer, (0.0, f64::from(mode.size.h.saturating_sub(height)))))
    }

    /// The ticker line for `entries`, built once per entry list.
    fn ticker_text(&mut self, entries: &std::sync::Arc<[ticker::Entry]>) -> std::sync::Arc<str> {
        if self
            .ticker_text
            .as_ref()
            .is_none_or(|(built, _)| built != entries)
        {
            let text = entries
                .iter()
                .map(|entry| entry.text(crate::pictograms::drawable))
                .collect::<Vec<_>>()
                .join(TICKER_SEPARATOR);
            self.ticker_text = Some((entries.clone(), text.into()));
        }
        self.ticker_text
            .as_ref()
            .map_or_else(|| "".into(), |(_, text)| text.clone())
    }
}

/// Where the chips, the ticker and the clock sit along the bar, in pixels.
struct Layout {
    unit: usize,
    width: usize,
    clock_start: usize,
    arrow: usize,
    chips: Vec<(usize, usize)>,
    ticker: (usize, usize),
}

impl Layout {
    fn new(segments: &[Segment], width: usize, unit: usize) -> Self {
        let clock_width = 104usize.saturating_mul(unit).min(width);
        let clock_start = width.saturating_sub(clock_width);
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
            chips.push((next_start, end));
            next_start = end;
        }
        let margin = 8usize.saturating_mul(unit);
        let ticker = (
            next_start.saturating_add(arrow).saturating_add(margin),
            clock_start.saturating_sub(margin),
        );
        Self {
            unit,
            width,
            clock_start,
            arrow,
            chips,
            ticker,
        }
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

fn raster(key: &Key, layout: &Layout, height: i32) -> Option<MemoryRenderBuffer> {
    let (segments, clock, width, scale, _, ticker) = key;
    let height_usize = usize::try_from(height).ok()?;
    let mut canvas = Canvas {
        width: layout.width,
        height: height_usize,
        pixels: vec![0; layout.width.checked_mul(height_usize)?.checked_mul(4)?],
    };
    for pixel in canvas.pixels.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[30, 24, 20, 255]);
    }
    let unit = layout.unit;
    canvas.rectangle(layout.clock_start, layout.width, colour(Tone::Context));
    canvas.text(
        clock,
        layout.clock_start.saturating_add(16usize.saturating_mul(unit)),
        layout.width,
        unit,
    );
    if let Some((text, offset)) = ticker {
        canvas.ticker(text, *offset, layout.ticker, unit);
    }
    for ((start, end), segment) in layout.chips.iter().zip(segments).rev() {
        canvas.chip(*start, *end, segment, layout.arrow, layout.clock_start, unit);
    }
    Some(MemoryRenderBuffer::from_slice(
        &canvas.pixels,
        Fourcc::Argb8888,
        (*width, height),
        *scale,
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
                crate::pictograms::glyph(character).unwrap_or([255, 129, 129, 129, 129, 129, 129, 255])
            };
            for (column, row) in lit(glyph) {
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

    /// Draws `text` in one-unit glyphs inside `clip`, shifted left by `offset`, with a
    /// repeat one text width plus the repeat gap later so the line loops.
    fn ticker(&mut self, text: &str, offset: usize, clip: (usize, usize), unit: usize) {
        let (start, end) = clip;
        if end.saturating_sub(start) < 32usize.saturating_mul(unit) {
            return;
        }
        let advance = 8usize.saturating_mul(unit);
        let glyphs: Vec<[u8; 8]> = crate::pictograms::glyphs(text).collect();
        let period = glyphs
            .len()
            .saturating_mul(advance)
            .saturating_add(ticker::REPEAT_GAP.saturating_mul(unit));
        let top = self.height.saturating_sub(advance) / 2;
        for copy in 0..2usize {
            for (index, glyph) in glyphs.iter().enumerate() {
                let left = index
                    .saturating_mul(advance)
                    .saturating_add(copy.saturating_mul(period));
                if left.saturating_add(advance) <= offset
                    || left >= offset.saturating_add(end.saturating_sub(start))
                {
                    continue;
                }
                for (column, row) in lit(*glyph) {
                    for vertical in 0..unit {
                        for horizontal in 0..unit {
                            let shifted = left
                                .saturating_add(column.saturating_mul(unit))
                                .saturating_add(horizontal);
                            let Some(on_bar) = shifted
                                .checked_sub(offset)
                                .map(|shift| start.saturating_add(shift))
                            else {
                                continue;
                            };
                            if on_bar < end {
                                self.pixel(
                                    on_bar,
                                    top.saturating_add(row.saturating_mul(unit))
                                        .saturating_add(vertical),
                                    [196, 184, 168, 255],
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The lit pixels of an 8x8 glyph as (column, row).
fn lit(glyph: [u8; 8]) -> impl Iterator<Item = (usize, usize)> {
    glyph.into_iter().enumerate().flat_map(|(row, bits)| {
        (0..8usize).filter_map(move |column| {
            let mask = 1u8
                .checked_shl(u32::try_from(column).unwrap_or_default())
                .unwrap_or_default();
            (bits & mask != 0).then_some((column, row))
        })
    })
}
