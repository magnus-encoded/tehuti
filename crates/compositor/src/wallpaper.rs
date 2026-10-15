//! File wallpaper, cached at each output size.

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

#[derive(Debug)]
pub struct Wallpaper {
    image: Option<image::DynamicImage>,
    buffers: std::collections::HashMap<(i32, i32), MemoryRenderBuffer>,
}

impl Wallpaper {
    #[must_use]
    pub fn load(path: &std::path::Path) -> Self {
        let image = match image::open(path) {
            Ok(image) => Some(image),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "wallpaper unavailable; using near-black");
                None
            }
        };
        Self {
            image,
            buffers: std::collections::HashMap::new(),
        }
    }

    /// # Errors
    /// Returns an error when output dimensions are not positive.
    pub fn buffer(&mut self, size: Size<i32, Physical>) -> Result<MemoryRenderBuffer, Box<dyn Error>> {
        let width = u32::try_from(size.w)?;
        let height = u32::try_from(size.h)?;
        if width == 0 || height == 0 {
            return Err("wallpaper size must be positive".into());
        }
        if let Some(buffer) = self.buffers.get(&(size.w, size.h)) {
            return Ok(buffer.clone());
        }
        let mut pixels = self.image.as_ref().map_or_else(
            || image::RgbaImage::from_pixel(width, height, image::Rgba([12, 12, 12, 255])),
            |image| {
                image
                    .resize_to_fill(width, height, image::imageops::FilterType::Lanczos3)
                    .to_rgba8()
            },
        );
        for pixel in pixels.pixels_mut() {
            let alpha = u16::from(pixel.0.last().copied().unwrap_or(255));
            for channel in pixel.0.iter_mut().take(3) {
                *channel = u8::try_from(u16::from(*channel).saturating_mul(alpha) / 255)?;
            }
        }
        let buffer = MemoryRenderBuffer::from_slice(
            pixels.as_raw(),
            Fourcc::Abgr8888,
            (size.w, size.h),
            1,
            Transform::Normal,
            Some(vec![Rectangle::from_size((size.w, size.h).into())]),
        );
        let _previous = self.buffers.insert((size.w, size.h), buffer.clone());
        Ok(buffer)
    }
}

fn wallpaper() -> &'static Mutex<Wallpaper> {
    static WALLPAPER: std::sync::OnceLock<Mutex<Wallpaper>> = std::sync::OnceLock::new();
    WALLPAPER.get_or_init(|| {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
            });
        let path = data.map(|data| data.join("tehuti/wallpaper.png"));
        Mutex::new(Wallpaper::load(path.as_deref().unwrap_or_else(|| {
            std::path::Path::new("/nonexistent/tehuti/wallpaper.png")
        })))
    })
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
    let buffer = wallpaper()
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
    let settings = crate::settings::element(renderer, compositor, output)?;
    if let Some(settings) = &settings {
        elements.push(OutputRenderElements::Custom(settings));
    }
    elements.extend(space.into_iter().map(OutputRenderElements::Space));
    elements.push(OutputRenderElements::Custom(&background));
    Ok(damage.render_output(renderer, framebuffer, 0, &elements, [0.0, 0.0, 0.0, 1.0])?)
}
