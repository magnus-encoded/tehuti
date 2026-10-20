//! File wallpaper, cached at each output size.

use core::error::Error;
use std::sync::Mutex;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Frame, ImportAll, ImportMem, Renderer,
            damage::{OutputDamageTracker, RenderOutputResult},
            element::{
                Element, Id, Kind, RenderElement,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    desktop::space::{SpaceRenderElements, space_render_elements},
    output::Output,
    utils::{Buffer, Physical, Rectangle, Scale, Size, Transform},
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
                tracing::warn!(%error, path = %path.display(), "wallpaper unavailable; using black");
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
            || image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255])),
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

#[derive(Debug)]
pub struct WallpaperCache {
    requests: std::sync::mpsc::SyncSender<Size<i32, Physical>>,
    buffers: std::sync::mpsc::Receiver<(Size<i32, Physical>, Option<MemoryRenderBuffer>)>,
    ready: std::collections::HashMap<(i32, i32), Option<MemoryRenderBuffer>>,
    pending: std::collections::HashSet<(i32, i32)>,
}

impl WallpaperCache {
    #[must_use]
    pub fn new(path: std::path::PathBuf) -> Self {
        let (requests, sizes) = std::sync::mpsc::sync_channel::<Size<i32, Physical>>(1);
        let (prepared, buffers) = std::sync::mpsc::channel();
        if let Err(error) = std::thread::Builder::new()
            .name("wallpaper".into())
            .spawn(move || {
                let mut wallpaper = Wallpaper::load(&path);
                while let Ok(size) = sizes.recv() {
                    let buffer = if wallpaper.image.is_some() {
                        match wallpaper.buffer(size) {
                            Ok(buffer) => Some(buffer),
                            Err(error) => {
                                tracing::warn!(%error, "wallpaper preparation failed; using black");
                                None
                            }
                        }
                    } else {
                        None
                    };
                    if prepared.send((size, buffer)).is_err() {
                        break;
                    }
                }
            })
        {
            tracing::warn!(%error, "wallpaper worker unavailable; using black");
        }
        Self {
            requests,
            buffers,
            ready: std::collections::HashMap::new(),
            pending: std::collections::HashSet::new(),
        }
    }

    pub fn buffer(&mut self, size: Size<i32, Physical>) -> Option<MemoryRenderBuffer> {
        while let Ok((prepared_size, buffer)) = self.buffers.try_recv() {
            let _removed = self.pending.remove(&(prepared_size.w, prepared_size.h));
            let _previous = self.ready.insert((prepared_size.w, prepared_size.h), buffer);
        }
        if let Some(buffer) = self.ready.get(&(size.w, size.h)) {
            return buffer.clone();
        }
        if self.pending.insert((size.w, size.h)) {
            match self.requests.try_send(size) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    tracing::warn!("wallpaper worker disconnected; using black");
                    let _previous = self.ready.insert((size.w, size.h), None);
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    let _removed = self.pending.remove(&(size.w, size.h));
                }
            }
        }
        None
    }
}

fn wallpaper() -> &'static Mutex<WallpaperCache> {
    static WALLPAPER: std::sync::OnceLock<Mutex<WallpaperCache>> = std::sync::OnceLock::new();
    WALLPAPER.get_or_init(|| {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
            });
        let path = data.map_or_else(
            || std::path::PathBuf::from("/nonexistent/tehuti/wallpaper.png"),
            |data| data.join("tehuti/wallpaper.png"),
        );
        Mutex::new(WallpaperCache::new(path))
    })
}

#[derive(Debug)]
pub struct WallpaperElement<E>(pub E);

impl<E: Element> Element for WallpaperElement<E> {
    fn id(&self) -> &Id {
        self.0.id()
    }
    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        self.0.current_commit()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.0.src()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.0.transform()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for WallpaperElement<E> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        if let Err(error) = self.0.draw(frame, src, dst, damage, opaque_regions) {
            tracing::warn!(%error, "wallpaper draw failed; using black");
            let clear = damage
                .iter()
                .map(|region| {
                    Rectangle::new(
                        (
                            region.loc.x.saturating_add(dst.loc.x),
                            region.loc.y.saturating_add(dst.loc.y),
                        )
                            .into(),
                        region.size,
                    )
                })
                .collect::<Vec<_>>();
            if let Err(clear_error) = frame.clear([0.0, 0.0, 0.0, 1.0].into(), &clear) {
                tracing::warn!(%clear_error, "wallpaper fallback clear failed");
            }
        }
        Ok(())
    }
}

smithay::backend::renderer::element::render_elements! {
    OutputRenderElements<'a, R, E, C> where R: ImportAll + ImportMem;
    Space=SpaceRenderElements<R, E>,
    Custom=&'a C,
    Wallpaper=WallpaperElement<MemoryRenderBufferRenderElement<R>>,
}

/// # Errors
/// Returns an error when window composition or output rendering fails.
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
    render_output_with_cache(
        output,
        renderer,
        framebuffer,
        compositor,
        overlays,
        damage,
        wallpaper(),
    )
}

/// # Errors
/// Returns an error when window composition or output rendering fails.
pub fn render_output_with_cache<'damage, R>(
    output: &Output,
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    compositor: &Compositor,
    overlays: &[MemoryRenderBufferRenderElement<R>],
    damage: &'damage mut OutputDamageTracker,
    cache: &Mutex<WallpaperCache>,
) -> Result<RenderOutputResult<'damage>, Box<dyn Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: 'static,
{
    let background = background(output, renderer, cache).unwrap_or_else(|error| {
        tracing::warn!(%error, "wallpaper render failed; using black");
        None
    });
    let space = space_render_elements(renderer, [&compositor.space], output, 1.0)?;
    let mut elements = overlays
        .iter()
        .map(OutputRenderElements::Custom)
        .collect::<Vec<_>>();
    elements.extend(space.into_iter().map(OutputRenderElements::Space));
    if let Some(background) = background {
        elements.push(OutputRenderElements::Wallpaper(WallpaperElement(background)));
    }
    Ok(damage.render_output(renderer, framebuffer, 0, &elements, [0.0, 0.0, 0.0, 1.0])?)
}

fn background<R>(
    output: &Output,
    renderer: &mut R,
    cache: &Mutex<WallpaperCache>,
) -> Result<Option<MemoryRenderBufferRenderElement<R>>, Box<dyn Error>>
where
    R: Renderer + ImportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: 'static,
{
    let size = output
        .current_transform()
        .transform_size(output.current_mode().ok_or("output has no mode")?.size);
    let Some(buffer) = cache
        .try_lock()
        .map_err(|_| "wallpaper cache unavailable")?
        .buffer(size)
    else {
        return Ok(None);
    };
    Ok(Some(MemoryRenderBufferRenderElement::from_buffer(
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
    )?))
}
