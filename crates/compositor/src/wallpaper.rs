//! Procedural release artwork, cached at the output's physical size.

use core::error::Error;
use std::sync::Mutex;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportAll, ImportMem, Renderer,
            damage::{OutputDamageTracker, RenderOutputResult},
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    desktop::space::{SpaceRenderElements, space_render_elements},
    output::Output,
    utils::{Physical, Rectangle, Size, Transform},
};

use crate::state::Compositor;

pub const RELEASE_CODENAME: &str = "Apteribis";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artwork {
    Apteribis,
}

#[must_use]
pub fn for_codename(codename: &str) -> Option<Artwork> {
    match codename {
        "Apteribis" => Some(Artwork::Apteribis),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct Cache {
    buffer: Option<(Size<i32, Physical>, MemoryRenderBuffer)>,
}

impl Cache {
    fn buffer(&mut self, size: Size<i32, Physical>) -> Result<MemoryRenderBuffer, Box<dyn Error>> {
        if let Some((cached_size, buffer)) = &self.buffer
            && *cached_size == size
        {
            return Ok(buffer.clone());
        }
        let artwork = for_codename(RELEASE_CODENAME).ok_or("unknown release wallpaper")?;
        let buffer = artwork.buffer(size)?;
        self.buffer = Some((size, buffer.clone()));
        Ok(buffer)
    }
}

impl Artwork {
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "positive i32 dimensions bound pixel coordinates and offsets"
    )]
    /// # Errors
    /// Returns an error for invalid dimensions or allocation failure.
    pub fn buffer(self, size: Size<i32, Physical>) -> Result<MemoryRenderBuffer, Box<dyn Error>> {
        let width = usize::try_from(size.w)?;
        let height = usize::try_from(size.h)?;
        if width == 0 || height == 0 {
            return Err("wallpaper size must be positive".into());
        }
        let length = width
            .checked_mul(height)
            .and_then(|area| area.checked_mul(4))
            .ok_or("wallpaper size overflow")?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(length)?;
        pixels.resize(length, 0);
        let unit = i64::from(size.w.min(size.h));
        for (offset, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let xpos = i64::try_from(offset % width)?;
            let ypos = i64::try_from(offset / width)?;
            let horizontal = (xpos - i64::from(size.w) / 2) * 1000 / unit + 500;
            let vertical = (ypos - i64::from(size.h) / 2) * 1000 / unit + 500;
            let color = match self {
                Self::Apteribis => apteribis_pixel(horizontal, vertical),
            };
            pixel.copy_from_slice(&color);
            if label_pixel(xpos, ypos, i64::from(size.h)) {
                pixel.copy_from_slice(&[89, 185, 232, 255]);
            }
        }
        Ok(MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (size.w, size.h),
            1,
            Transform::Normal,
            Some(vec![Rectangle::from_size((size.w, size.h).into())]),
        ))
    }
}

#[expect(
    clippy::arithmetic_side_effects,
    reason = "coordinates are bounded by positive i32 output dimensions"
)]
const fn ellipse(horizontal: i64, vertical: i64, center: (i64, i64), radius: (i64, i64)) -> bool {
    let delta_x = horizontal - center.0;
    let delta_y = vertical - center.1;
    delta_x * delta_x * radius.1 * radius.1 + delta_y * delta_y * radius.0 * radius.0
        <= radius.0 * radius.0 * radius.1 * radius.1
}

#[expect(
    clippy::arithmetic_side_effects,
    reason = "artwork coordinates are bounded by i32 output dimensions"
)]
fn apteribis_pixel(horizontal: i64, vertical: i64) -> [u8; 4] {
    if !(0..=1000).contains(&horizontal) || !(0..=1000).contains(&vertical) {
        return [20, 15, 11, 255];
    }
    let body = ellipse(horizontal, vertical, (440, 510), (160, 100));
    let neck = ellipse(horizontal, vertical, (570, 420), (45, 130));
    let head = ellipse(horizontal, vertical, (605, 305), (55, 42));
    let bill = (630..=805).contains(&horizontal)
        && vertical >= 305 + (horizontal - 630) * (horizontal - 630) / 260
        && vertical <= 325 + (horizontal - 630) * (horizontal - 630) / 260;
    let legs = ((410..=425).contains(&horizontal) || (495..=510).contains(&horizontal))
        && (580..=775).contains(&vertical);
    let feet = (400..=545).contains(&horizontal) && (770..=780).contains(&vertical);
    if body || neck || head || bill || legs || feet {
        return [24, 29, 34, 255];
    }
    if ellipse(horizontal, vertical, (520, 450), (320, 320)) {
        return [90, 66, 20, 255];
    }
    if (800..=803).contains(&vertical) && (220..=810).contains(&horizontal) {
        return [89, 185, 232, 255];
    }
    [20, 15, 11, 255]
}

#[expect(
    clippy::arithmetic_side_effects,
    reason = "label coordinates and glyph indices are bounded"
)]
fn label_pixel(xpos: i64, ypos: i64, height: i64) -> bool {
    let column = (xpos - 24) / 2;
    let row = (ypos - (height - 38)) / 2;
    if xpos < 24 || ypos < height - 38 || !(0..7).contains(&row) || !(0..54).contains(&column) {
        return false;
    }
    let glyphs = [
        [14_u8, 17, 17, 31, 17, 17, 17],
        [0, 0, 30, 17, 30, 16, 16],
        [8, 8, 28, 8, 8, 9, 6],
        [0, 0, 14, 17, 31, 16, 14],
        [0, 0, 22, 25, 16, 16, 16],
        [4, 0, 12, 4, 4, 4, 14],
        [16, 16, 22, 25, 17, 17, 30],
        [4, 0, 12, 4, 4, 4, 14],
        [0, 0, 15, 16, 14, 1, 30],
    ];
    glyphs
        .get(usize::try_from(column / 6).unwrap_or_default())
        .and_then(|glyph| glyph.get(usize::try_from(row).unwrap_or_default()))
        .is_some_and(|bits| column % 6 < 5 && bits & (16 >> (column % 6)) != 0)
}

smithay::backend::renderer::element::render_elements! {
    OutputRenderElements<'a, R, E, C> where R: ImportAll;
    Space=SpaceRenderElements<R, E>,
    Custom=&'a C,
}

/// # Errors
/// Returns an error when the output, cache, buffer import or rendering fails.
pub fn render_output<'damage, R>(
    output: &Output,
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    compositor: &Compositor,
    overlays: &[MemoryRenderBufferRenderElement<R>],
    damage: &'damage mut OutputDamageTracker,
) -> Result<RenderOutputResult<'damage>, Box<dyn Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: 'static,
{
    let size = output
        .current_transform()
        .transform_size(output.current_mode().ok_or("output has no mode")?.size);
    let _inserted = output
        .user_data()
        .insert_if_missing(|| Mutex::new(Cache::default()));
    let buffer = output
        .user_data()
        .get::<Mutex<Cache>>()
        .ok_or("missing wallpaper cache")?
        .lock()
        .map_err(|_| "wallpaper cache poisoned")?
        .buffer(size)?;
    let background = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        &buffer,
        None,
        Some(Rectangle::from_size((size.w, size.h).into()).to_f64()),
        Some(
            size.to_f64()
                .to_logical(output.current_scale().fractional_scale())
                .to_i32_round(),
        ),
        Kind::Unspecified,
    )?;
    let space = space_render_elements(renderer, [&compositor.space], output, 1.0)?;
    let mut elements = overlays
        .iter()
        .map(OutputRenderElements::Custom)
        .collect::<Vec<_>>();
    elements.extend(space.into_iter().map(OutputRenderElements::Space));
    elements.push(OutputRenderElements::Custom(&background));
    Ok(damage.render_output(renderer, framebuffer, 0, &elements, [0.0, 0.0, 0.0, 1.0])?)
}

#[cfg(test)]
mod tests {
    use super::{Artwork, for_codename};
    use smithay::backend::renderer::{
        element::{Element, Kind, memory::MemoryRenderBufferRenderElement},
        pixman::PixmanRenderer,
    };

    #[test]
    fn release_codename_selects_its_artwork() {
        assert_eq!(for_codename("Apteribis"), Some(Artwork::Apteribis));
        assert_eq!(for_codename("Bostrychia"), None);
        assert_eq!(for_codename("unknown"), None);
    }

    #[test]
    fn buffer_matches_output_dimensions() -> Result<(), Box<dyn core::error::Error>> {
        let mut buffer = Artwork::Apteribis.buffer((320, 180).into())?;
        buffer.render().draw(|pixels| {
            assert_eq!(pixels.len(), 230_400);
            Ok::<_, Box<dyn core::error::Error>>(Vec::new())
        })?;
        let element = MemoryRenderBufferRenderElement::from_buffer(
            &mut PixmanRenderer::new()?,
            (0.0, 0.0),
            &buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )?;
        assert_eq!(element.geometry(1.0.into()).size, (320, 180).into());
        assert!(Artwork::Apteribis.buffer((0, 180).into()).is_err());
        Ok(())
    }
}
