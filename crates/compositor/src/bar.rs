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
    Option<(std::sync::Arc<[ticker::Entry]>, usize)>,
);

#[derive(Debug, Default)]
pub struct Buffer {
    key: Option<Key>,
    image: Option<MemoryRenderBuffer>,
    ticker_strip: Option<(
        std::sync::Arc<[ticker::Entry]>,
        std::sync::Arc<crate::ticker_strip::Strip>,
    )>,
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
            let content = self.ticker_strip(&view.entries).width().saturating_mul(unit);
            let offset = ticker::scroll_offset(
                view.step,
                content,
                layout.ticker.1.saturating_sub(layout.ticker.0),
                ticker::STEP_PIXELS.saturating_mul(unit),
                ticker::REPEAT_GAP.saturating_mul(unit),
            );
            (view.entries, offset)
        });
        let key = (segments, clock.to_owned(), mode.size.w, scale, height, ticker);
        if self.key.as_ref() != Some(&key) {
            self.image = raster(
                &key,
                &layout,
                height,
                self.ticker_strip.as_ref().map(|(_, strip)| &**strip),
            );
            self.key = Some(key);
        }
        self.image
            .as_ref()
            .map(|buffer| (buffer, (0.0, f64::from(mode.size.h.saturating_sub(height)))))
    }

    /// The ticker drawn for `entries`, painted once per entry list.
    fn ticker_strip(
        &mut self,
        entries: &std::sync::Arc<[ticker::Entry]>,
    ) -> std::sync::Arc<crate::ticker_strip::Strip> {
        match &self.ticker_strip {
            Some((built, strip)) if built == entries => strip.clone(),
            _ => {
                let strip = std::sync::Arc::new(crate::ticker_strip::Strip::paint(entries));
                self.ticker_strip = Some((entries.clone(), strip.clone()));
                strip
            }
        }
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

fn raster(
    key: &Key,
    layout: &Layout,
    height: i32,
    strip: Option<&crate::ticker_strip::Strip>,
) -> Option<MemoryRenderBuffer> {
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
    if let (Some((_, offset)), Some(strip)) = (ticker, strip) {
        canvas.ticker(strip, *offset, layout.ticker, unit);
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
            for (column, row) in crate::pictograms::lit(glyph) {
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

    /// Copies the ticker strip into `clip`, scaled by `unit` and shifted left by
    /// `offset`, with a repeat one strip width plus the repeat gap later so the line loops.
    fn ticker(
        &mut self,
        strip: &crate::ticker_strip::Strip,
        offset: usize,
        clip: (usize, usize),
        unit: usize,
    ) {
        let (start, end) = clip;
        if end.saturating_sub(start) < 32usize.saturating_mul(unit) || unit == 0 {
            return;
        }
        let height = crate::ticker_strip::HEIGHT.saturating_mul(unit);
        let top = self.height.saturating_sub(height) / 2;
        let period = strip
            .width()
            .saturating_add(ticker::REPEAT_GAP)
            .saturating_mul(unit);
        for row in 0..height {
            let strip_row = row.checked_div(unit).unwrap_or_default();
            for column in start..end {
                let along = column
                    .saturating_sub(start)
                    .saturating_add(offset)
                    .checked_rem(period)
                    .unwrap_or_default();
                if let Some(colour) = strip.pixel(along.checked_div(unit).unwrap_or_default(), strip_row) {
                    self.pixel(column, top.saturating_add(row), bgra(colour));
                }
            }
        }
    }
}

/// A colour in the bar's byte order, Argb8888 stored little-endian.
const fn bgra(colour: ticker::Rgb) -> [u8; 4] {
    let ticker::Rgb(red, green, blue) = colour;
    [blue, green, red, 255]
}
