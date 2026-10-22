//! Native-resolution coverage rasterisation for settings artwork.

use core::time::Duration;
use fontdue::{
    Font, FontSettings,
    layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle},
};

#[expect(
    clippy::redundant_pub_crate,
    reason = "the canvas is shared with wallpaper but stays inside the crate"
)]
pub(crate) struct Canvas {
    pub(crate) pixels: Vec<u8>,
    width: usize,
    height: usize,
    scale: (f32, f32),
    font: Font,
}

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
        let font = Font::from_bytes(
            include_bytes!("../../assets/LiberationSans-Regular.ttf") as &[u8],
            FontSettings::default(),
        )
        .ok()?;
        Some(Self {
            pixels: vec![0; length],
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
            width,
            height,
            scale: (1.0, 1.0),
            font: Font::from_bytes(
                include_bytes!("../../assets/LiberationSans-Regular.ttf") as &[u8],
                FontSettings::default(),
            )
            .ok()?,
        })
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
            let [_, _, _, opacity] = color;
            let alpha = f32::from(opacity) / 255.0 * coverage;
            for (channel, source) in pixel.iter_mut().take(3).zip(color) {
                *channel = f32::from(*channel)
                    .mul_add(1.0 - alpha, f32::from(source) * alpha)
                    .round() as u8;
            }
            if let Some(channel) = pixel.get_mut(3) {
                *channel = f32::from(*channel).mul_add(1.0 - alpha, 255.0 * alpha).round() as u8;
            }
        }
    }

    pub(crate) fn rectangle(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
        let left = x as f32 * self.scale.0;
        let top = y as f32 * self.scale.1;
        let right = x.saturating_add(width) as f32 * self.scale.0;
        let bottom = y.saturating_add(height) as f32 * self.scale.1;
        for row in (top.floor() as usize)..(bottom.ceil() as usize).min(self.height) {
            for column in (left.floor() as usize)..(right.ceil() as usize).min(self.width) {
                let coverage = (right.min(column as f32 + 1.0) - left.max(column as f32)).clamp(0.0, 1.0)
                    * (bottom.min(row as f32 + 1.0) - top.max(row as f32)).clamp(0.0, 1.0);
                self.blend(column, row, color, coverage);
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
            &[&self.font],
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
