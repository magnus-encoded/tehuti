//! Fixed-cell powerline text, with bitmap fallback for unavailable fonts and symbols.

use fontdue::{Font, FontSettings};
use std::sync::LazyLock;

static MONO: LazyLock<Option<Font>> = LazyLock::new(|| {
    let loaded = std::env::var_os("TEHUTI_BAR_FONT_FILE").map_or_else(
        || {
            Font::from_bytes(
                include_bytes!("../assets/LiberationMono-Regular.ttf").as_slice(),
                FontSettings::default(),
            )
            .map_err(std::io::Error::other)
        },
        |path| load(std::path::Path::new(&path)),
    );
    match loaded {
        Ok(font) => Some(font),
        Err(error) => {
            tracing::warn!(%error, "Liberation Mono unavailable; using bitmap bar font");
            None
        }
    }
});

fn load(path: &std::path::Path) -> std::io::Result<Font> {
    Font::from_bytes(std::fs::read(path)?, FontSettings::default()).map_err(std::io::Error::other)
}

/// Coverage in a fixed cell, sharing a baseline across every character.
#[expect(
    clippy::redundant_pub_crate,
    reason = "the text raster stays private to the compositor"
)]
pub(crate) fn coverage(character: char, choice: tehuti_core::bar::Font, large: bool) -> Vec<u8> {
    let (width, height, size, baseline) = if large {
        (16usize, 24usize, 20.0, 18i32)
    } else {
        (8, 12, 10.0, 8)
    };
    let mut pixels = vec![0; width.saturating_mul(height)];
    if choice == tehuti_core::bar::Font::LiberationMono
        && let Some(font) = &*MONO
        && font.lookup_glyph_index(character) != 0
    {
        let (metrics, bitmap) = font.rasterize(character, size);
        let left = width.saturating_sub(metrics.width) / 2;
        let top = baseline
            .saturating_sub(metrics.ymin)
            .saturating_sub(i32::try_from(metrics.height).unwrap_or_default());
        for (index, alpha) in bitmap.into_iter().enumerate() {
            let column = left.saturating_add(index.checked_rem(metrics.width).unwrap_or_default());
            let row = top.saturating_add(
                i32::try_from(index.checked_div(metrics.width).unwrap_or_default()).unwrap_or_default(),
            );
            if let Ok(row) = usize::try_from(row)
                && column < width
                && row < height
                && let Some(pixel) = pixels.get_mut(row.saturating_mul(width).saturating_add(column))
            {
                *pixel = alpha;
            }
        }
    } else {
        let scale = if large { 2usize } else { 1 };
        let top = if large { 4usize } else { 2 };
        let glyph = if character == '…' {
            [0, 0, 0, 0, 0, 0, 42, 0]
        } else {
            crate::pictograms::glyph(character).unwrap_or([255, 129, 129, 129, 129, 129, 129, 255])
        };
        for (column, row) in crate::pictograms::lit(glyph) {
            for vertical in 0..scale {
                for horizontal in 0..scale {
                    let row = top
                        .saturating_add(row.saturating_mul(scale))
                        .saturating_add(vertical);
                    let column = column.saturating_mul(scale).saturating_add(horizontal);
                    if let Some(pixel) = pixels.get_mut(row.saturating_mul(width).saturating_add(column)) {
                        *pixel = 255;
                    }
                }
            }
        }
    }
    pixels
}
