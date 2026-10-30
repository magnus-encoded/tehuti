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
    tehuti_core::Level,
    String,
    i32,
    i32,
    i32,
    Option<(std::sync::Arc<[ticker::Entry]>, usize)>,
    tehuti_core::bar::Font,
);

#[derive(Debug, Default)]
pub struct Buffer {
    key: Option<Key>,
    font: tehuti_core::bar::Font,
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
        if self.font != bar.font {
            self.font = bar.font;
            self.ticker_strip = None;
        }
        let mode = output.current_mode()?;
        let scale = output.current_scale().integer_scale().max(1);
        let height = tehuti_core::bar::HEIGHT.saturating_mul(scale).min(mode.size.h);
        let segments = bar.segments();
        let unit = usize::try_from(scale).ok()?;
        let width = usize::try_from(mode.size.w).ok()?;
        let layout = Layout::new(&segments, width, unit, bar.level(), bar.focused_window_segment());
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
        let key = (
            segments,
            bar.level(),
            clock.to_owned(),
            mode.size.w,
            scale,
            height,
            ticker,
            self.font,
        );
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
                let strip =
                    std::sync::Arc::new(crate::ticker_strip::Strip::paint_with_font(entries, self.font));
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
    chips: Vec<(usize, usize, usize)>,
    scroll: usize,
    ticker: (usize, usize),
}

impl Layout {
    fn new(
        segments: &[Segment],
        width: usize,
        unit: usize,
        level: tehuti_core::Level,
        focused: Option<usize>,
    ) -> Self {
        let clock_width = 104usize.saturating_mul(unit).min(width);
        let clock_start = width.saturating_sub(clock_width);
        if level == tehuti_core::Level::Columns {
            return Self::columns(segments, width, unit, clock_start, focused);
        }
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
            chips.push((index, next_start, end));
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
            scroll: 0,
            ticker,
        }
    }

    fn columns(
        segments: &[Segment],
        width: usize,
        unit: usize,
        clock_start: usize,
        focused: Option<usize>,
    ) -> Self {
        let mut next = 0usize;
        let chips: Vec<_> = segments
            .iter()
            .enumerate()
            .map(|(index, segment)| {
                let start = next;
                let wanted = segment
                    .text
                    .chars()
                    .count()
                    .saturating_mul(16)
                    .saturating_add(36)
                    .saturating_mul(unit)
                    .min(clock_start);
                next = next.saturating_add(wanted);
                (index, start, next)
            })
            .collect();
        let scroll = chips
            .iter()
            .find(|(index, _, _)| Some(*index) == focused)
            .map_or(0, |(_, _, end)| end.saturating_sub(clock_start));
        let ticker = (
            next.saturating_sub(scroll)
                .saturating_add(8usize.saturating_mul(unit)),
            clock_start.saturating_sub(8usize.saturating_mul(unit)),
        );
        Self {
            unit,
            width,
            clock_start,
            arrow: 0,
            chips,
            scroll,
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
    let (segments, _, clock, width, scale, _, ticker, font) = key;
    let height_usize = usize::try_from(height).ok()?;
    let mut canvas = Canvas {
        font: *font,
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
    for (index, start, end) in layout.chips.iter().rev() {
        let Some(segment) = segments.get(*index) else {
            continue;
        };
        if key.1 == tehuti_core::Level::Columns {
            let limit = layout.scroll.saturating_add(layout.clock_start);
            let end = (*end).min(limit);
            canvas.rectangle(
                start.saturating_sub(layout.scroll).min(layout.clock_start),
                end.saturating_sub(layout.scroll),
                colour(segment.tone),
            );
            canvas.scrolled_text(
                &segment.text,
                (
                    start.saturating_add(18usize.saturating_mul(unit)),
                    end.saturating_sub(4usize.saturating_mul(unit)),
                ),
                unit,
                layout.scroll,
            );
        } else {
            canvas.chip(*start, *end, segment, layout.arrow, layout.clock_start, unit);
        }
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
    font: tehuti_core::bar::Font,
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
        self.scrolled_text(text, (start, end), unit, 0);
    }

    fn scrolled_text(&mut self, text: &str, clip: (usize, usize), unit: usize, offset: usize) {
        let (start, end) = clip;
        let advance = 16usize.saturating_mul(unit);
        let top = self.height.saturating_sub(24usize.saturating_mul(unit)) / 2;
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
            for (sample, alpha) in crate::bar_font::coverage(character, self.font, true)
                .into_iter()
                .enumerate()
            {
                if alpha == 0 {
                    continue;
                }
                for vertical in 0..unit {
                    for horizontal in 0..unit {
                        if let Some(column) = left
                            .saturating_add((sample % 16).saturating_mul(unit))
                            .saturating_add(horizontal)
                            .checked_sub(offset)
                        {
                            let row = top
                                .saturating_add((sample / 16).saturating_mul(unit))
                                .saturating_add(vertical);
                            self.blend(column, row, alpha);
                        }
                    }
                }
            }
        }
    }

    fn blend(&mut self, column: usize, row: usize, alpha: u8) {
        if column >= self.width || row >= self.height {
            return;
        }
        let offset = row
            .saturating_mul(self.width)
            .saturating_add(column)
            .saturating_mul(4);
        if let Some(pixel) = self.pixels.get_mut(offset..offset.saturating_add(4)) {
            for (channel, foreground) in pixel.iter_mut().take(3).zip([239u16, 235, 229]) {
                *channel = u8::try_from(
                    u16::from(*channel)
                        .saturating_mul(255u16.saturating_sub(u16::from(alpha)))
                        .saturating_add(foreground.saturating_mul(u16::from(alpha)))
                        / 255,
                )
                .unwrap_or_default();
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
