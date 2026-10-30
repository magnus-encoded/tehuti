//! Native-resolution coverage rasterisation for settings artwork.

use core::time::Duration;
use fontdue::{
    Font, FontSettings,
    layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle},
};

#[expect(
    clippy::redundant_pub_crate,
    reason = "the canvas is shared by desktop shell artwork"
)]
pub(crate) struct Canvas {
    pub(crate) pixels: Vec<u8>,
    pub(crate) trace: bool,
    pub(crate) glyphs: Vec<serde_json::Value>,
    width: usize,
    height: usize,
    scale: (f32, f32),
    font: &'static Font,
}

static FONT: std::sync::LazyLock<Option<Font>> = std::sync::LazyLock::new(|| {
    const BYTES: &[u8] = include_bytes!("../../assets/LiberationSans-Regular.ttf");
    Font::from_bytes(BYTES, FontSettings::default()).ok()
});

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "raster coordinates are bounded by the validated output dimensions"
)]
#[expect(
    clippy::float_arithmetic,
    reason = "coverage rasterisation uses fractional physical pixels"
)]
impl Canvas {
    pub(crate) fn new(size: (i32, i32)) -> Option<Self> {
        let width = usize::try_from(size.0).ok().filter(|value| *value > 0)?;
        let height = usize::try_from(size.1).ok().filter(|value| *value > 0)?;
        let length = width.checked_mul(height)?.checked_mul(4)?;
        let font = FONT.as_ref()?;
        Some(Self {
            pixels: vec![0; length],
            glyphs: Vec::new(),
            trace: false,
            width,
            height,
            scale: (width as f32 / 1280.0, height as f32 / 720.0),
            font,
        })
    }

    pub(crate) fn from_pixels(size: (i32, i32), pixels: Vec<u8>) -> Option<Self> {
        let width = usize::try_from(size.0).ok().filter(|value| *value > 0)?;
        let height = usize::try_from(size.1).ok().filter(|value| *value > 0)?;
        if width.checked_mul(height)?.checked_mul(4)? != pixels.len() {
            return None;
        }
        Some(Self {
            pixels,
            glyphs: Vec::new(),
            trace: false,
            width,
            height,
            scale: (1.0, 1.0),
            font: FONT.as_ref()?,
        })
    }

    pub(crate) fn reference(&mut self, width: usize, height: usize) {
        self.scale = (
            self.width as f32 / width as f32,
            self.height as f32 / height as f32,
        );
    }

    pub(crate) fn line(
        &mut self,
        bounds: (usize, usize, usize, usize),
        runs: &[(&str, [u8; 4])],
        size: usize,
    ) {
        let (left, top, width, height) = bounds;
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.reset(&LayoutSettings {
            x: left as f32 * self.scale.0,
            y: top as f32 * self.scale.1,
            ..LayoutSettings::default()
        });
        for (text, colour) in runs {
            layout.append(
                &[self.font],
                &TextStyle::with_user_data(text, size as f32 * self.scale.1, 0, *colour),
            );
        }
        for glyph in layout.glyphs() {
            let (_, coverage) = self.font.rasterize_config(glyph.key);
            for (index, alpha) in coverage.into_iter().enumerate() {
                let column =
                    (glyph.x as usize).saturating_add(index.checked_rem(glyph.width).unwrap_or_default());
                let row =
                    (glyph.y as usize).saturating_add(index.checked_div(glyph.width).unwrap_or_default());
                if column >= (left as f32 * self.scale.0) as usize
                    && column < (left.saturating_add(width) as f32 * self.scale.0) as usize
                    && row >= (top as f32 * self.scale.1) as usize
                    && row < (top.saturating_add(height) as f32 * self.scale.1) as usize
                {
                    self.blend(column, row, glyph.user_data, f32::from(alpha) / 255.0);
                }
            }
        }
    }

    pub(crate) fn record_glyph(&mut self, before: &[u8], circle: (usize, usize, usize)) {
        let mut left = self.width;
        let mut top = self.height;
        let mut right = 0;
        let mut bottom = 0;
        for (index, (old, new)) in before
            .as_chunks::<4>()
            .0
            .iter()
            .zip(self.pixels.as_chunks::<4>().0.iter())
            .enumerate()
        {
            if old != new {
                let x = index.checked_rem(self.width).unwrap_or_default();
                let y = index.checked_div(self.width).unwrap_or_default();
                left = left.min(x);
                top = top.min(y);
                right = right.max(x.saturating_add(1));
                bottom = bottom.max(y.saturating_add(1));
            }
        }
        let (x, y, diameter) = circle;
        self.glyphs.push(serde_json::json!({
            "ink": [left, top, right, bottom],
            "circle": [x as f32 * self.scale.0, y as f32 * self.scale.1,
                x.saturating_add(diameter) as f32 * self.scale.0,
                y.saturating_add(diameter) as f32 * self.scale.1]
        }));
    }

    pub(crate) fn image(&mut self, x: usize, y: usize, image: &image::RgbaImage) {
        for (column, row, pixel) in image.enumerate_pixels() {
            self.blend(
                x.saturating_add(column as usize),
                y.saturating_add(row as usize),
                pixel.0,
                1.0,
            );
        }
    }

    fn blend(&mut self, x: usize, y: usize, color: [u8; 4], coverage: f32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y.saturating_mul(self.width).saturating_add(x).saturating_mul(4);
        if let Some(pixel) = self.pixels.get_mut(offset..offset.saturating_add(4)) {
            blend_pixel(pixel, color, alpha(color, coverage));
        }
    }

    pub(crate) fn rectangle(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
        let left = x as f32 * self.scale.0;
        let top = y as f32 * self.scale.1;
        let right = x.saturating_add(width) as f32 * self.scale.0;
        let bottom = y.saturating_add(height) as f32 * self.scale.1;
        let first_column = (left.floor() as usize).min(self.width);
        let end_column = (right.ceil() as usize).min(self.width);
        let column_coverage: Vec<f32> = (first_column..end_column)
            .map(|column| (right.min(column as f32 + 1.0) - left.max(column as f32)).clamp(0.0, 1.0))
            .collect();
        let full = Lookup::new(color, alpha(color, 1.0));
        let full_row_alphas: Vec<u8> = column_coverage
            .iter()
            .map(|coverage| alpha(color, *coverage))
            .collect();
        for row in (top.floor() as usize)..(bottom.ceil() as usize).min(self.height) {
            let row_coverage = (bottom.min(row as f32 + 1.0) - top.max(row as f32)).clamp(0.0, 1.0);
            let start = row
                .saturating_mul(self.width)
                .saturating_add(first_column)
                .saturating_mul(4);
            let end = row
                .saturating_mul(self.width)
                .saturating_add(end_column)
                .saturating_mul(4);
            let Some(pixels) = self.pixels.get_mut(start..end) else {
                continue;
            };
            let partial_row_alphas: Vec<u8>;
            let row_alphas: &[u8] = if row_coverage >= 1.0 {
                &full_row_alphas
            } else {
                partial_row_alphas = column_coverage
                    .iter()
                    .map(|coverage| alpha(color, row_coverage * coverage))
                    .collect();
                &partial_row_alphas
            };
            for (pixel, &pixel_alpha) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(row_alphas) {
                if pixel_alpha == full.alpha {
                    full.apply(pixel);
                } else {
                    blend_pixel(pixel, color, pixel_alpha);
                }
            }
        }
    }

    pub(crate) fn text(&mut self, x: usize, y: usize, text: &str, scale: usize, color: [u8; 4]) {
        self.text_box((x, y, usize::MAX, usize::MAX), text, scale, color);
    }

    pub(crate) fn text_box(
        &mut self,
        bounds: (usize, usize, usize, usize),
        text: &str,
        scale: usize,
        color: [u8; 4],
    ) {
        let (x, y, width, height) = bounds;
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.reset(&LayoutSettings {
            x: x as f32 * self.scale.0,
            y: y as f32 * self.scale.1,
            max_width: (width != usize::MAX).then_some(width as f32 * self.scale.0),
            ..LayoutSettings::default()
        });
        layout.append(
            &[self.font],
            &TextStyle::new(text, scale as f32 * 8.0 * self.scale.1, 0),
        );
        for glyph in layout.glyphs() {
            let (_, coverage) = self.font.rasterize_config(glyph.key);
            for (index, alpha) in coverage.into_iter().enumerate() {
                let column = (glyph.x as usize).saturating_add(index.checked_rem(glyph.width).unwrap_or(0));
                let row = (glyph.y as usize).saturating_add(index.checked_div(glyph.width).unwrap_or(0));
                if column < (x.saturating_add(width) as f32 * self.scale.0) as usize
                    && row < (y.saturating_add(height) as f32 * self.scale.1) as usize
                {
                    self.blend(column, row, color, f32::from(alpha) / 255.0);
                }
            }
        }
    }

    pub(crate) fn thumbnail(&mut self, x: usize, y: usize, image: &image::RgbaImage, locked: bool) {
        for row in
            (y as f32 * self.scale.1) as usize..((y.saturating_add(135)) as f32 * self.scale.1) as usize
        {
            for column in
                (x as f32 * self.scale.0) as usize..((x.saturating_add(240)) as f32 * self.scale.0) as usize
            {
                let source_x = ((column as f32 / self.scale.0 - x as f32) as u32).min(239);
                let source_y = ((row as f32 / self.scale.1 - y as f32) as u32).min(134);
                let pixel = image.get_pixel(source_x, source_y).0;
                let [red, green, blue, alpha] = pixel;
                let dim = if locked { 3 } else { 1 };
                self.blend(
                    column,
                    row,
                    [
                        blue.checked_div(dim).unwrap_or_default(),
                        green.checked_div(dim).unwrap_or_default(),
                        red.checked_div(dim).unwrap_or_default(),
                        alpha,
                    ],
                    1.0,
                );
            }
        }
    }

    pub(crate) fn wave(&mut self, x: usize, y: usize, hertz: f32) {
        let wavelength = 2400.0 / hertz.clamp(1.0, 1000.0);
        for column in 0..150 {
            let offset = (column as f32 / wavelength * core::f32::consts::TAU).sin() * 10.0;
            self.rectangle(
                x.saturating_add(column),
                (y as f32 + offset) as usize,
                2,
                2,
                super::CYAN,
            );
        }
    }

    pub(crate) fn countdown(&mut self, remaining: Duration) {
        let fraction = remaining.as_secs_f32() / 15.0;
        let pixel_width = 1.0 / self.scale.0.min(self.scale.1);
        for row in (460.0 * self.scale.1) as usize..(640.0 * self.scale.1) as usize {
            for column in (1000.0 * self.scale.0) as usize..(1180.0 * self.scale.0) as usize {
                let delta_x = (column as f32 + 0.5) / self.scale.0 - 1090.0;
                let delta_y = (row as f32 + 0.5) / self.scale.1 - 550.0;
                let distance = (delta_x.hypot(delta_y) - 78.0).abs();
                let coverage = ((4.0 - distance) / pixel_width + 0.5).clamp(0.0, 1.0);
                let angle = delta_x.atan2(-delta_y).rem_euclid(core::f32::consts::TAU);
                let color = if angle / core::f32::consts::TAU <= fraction {
                    super::AMBER
                } else {
                    [42, 51, 54, 255]
                };
                self.blend(column, row, color, coverage);
            }
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "the product of an opacity and a clamped coverage lies within 0..=255"
)]
#[expect(
    clippy::float_arithmetic,
    reason = "coverage is a fraction of a physical pixel"
)]
fn alpha(color: [u8; 4], coverage: f32) -> u8 {
    let [_, _, _, opacity] = color;
    (f32::from(opacity) * coverage.clamp(0.0, 1.0)).round() as u8
}

/// Precomputed blends of one colour at one alpha, for the many fully covered pixels of a rectangle.
struct Lookup {
    alpha: u8,
    channels: [[u8; 256]; 4],
}

impl Lookup {
    fn new(color: [u8; 4], alpha: u8) -> Self {
        let mut channels = [[0; 256]; 4];
        for (destination, value) in (0..=u8::MAX).enumerate() {
            let mut pixel = [value; 4];
            blend_pixel(&mut pixel, color, alpha);
            for (channel, blended) in channels.iter_mut().zip(pixel) {
                if let Some(entry) = channel.get_mut(destination) {
                    *entry = blended;
                }
            }
        }
        Self { alpha, channels }
    }

    fn apply(&self, pixel: &mut [u8]) {
        for (channel, table) in pixel.iter_mut().zip(&self.channels) {
            *channel = table.get(usize::from(*channel)).copied().unwrap_or(*channel);
        }
    }
}

/// Integer source-over blending; `f32::mul_add` is a slow software call on CPUs without FMA.
fn blend_pixel(pixel: &mut [u8], color: [u8; 4], alpha: u8) {
    let alpha = u16::from(alpha);
    let mix = |destination: u8, source: u8| {
        let mixed = u16::from(destination)
            .saturating_mul(255_u16.saturating_sub(alpha))
            .saturating_add(u16::from(source).saturating_mul(alpha))
            .saturating_add(127)
            / 255;
        u8::try_from(mixed).unwrap_or(u8::MAX)
    };
    let [red, green, blue, _] = color;
    for (channel, source) in pixel.iter_mut().zip([red, green, blue, u8::MAX]) {
        *channel = mix(*channel, source);
    }
}
