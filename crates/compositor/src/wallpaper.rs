//! File wallpaper, cached at each output size.

use core::error::Error;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

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
use smithay::reexports::calloop::ping::Ping;

#[derive(Debug)]
struct Request {
    size: Size<i32, Physical>,
    redraw: Option<Ping>,
    image: Option<image::DynamicImage>,
    generation: u64,
}

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

pub const CODENAMES: &[&str] = &[
    "apteribis",
    "bostrychia",
    "cercibis",
    "eudocimus",
    "geronticus",
    "lophotibis",
    "mesembrinibis",
    "nipponia",
    "phimosus",
    "plegadis",
    "pseudibis",
    "theristicus",
    "threskiornis",
];
const CURRENT_RELEASE: usize = 0;

#[derive(Debug)]
pub struct Collection {
    locked: Vec<bool>,
    thumbnails: Vec<image::RgbaImage>,
    entries: Vec<(String, image::DynamicImage)>,
    pub(crate) current: Option<usize>,
    wallpaper: Wallpaper,
    store: crate::display_mode_store::Store,
}

impl Collection {
    #[must_use]
    pub fn discover(
        roots: &[PathBuf],
        store: crate::display_mode_store::Store,
        release: usize,
        all: bool,
    ) -> Self {
        let unlocked = store
            .sibling("wallpapers-unlocked")
            .read()
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "wallpaper unlock unavailable");
                None
            })
            .as_deref()
            == Some("1");
        let mut entries = load_entries(roots, release, true);
        let legacy = roots
            .iter()
            .find_map(|root| image::open(root.join("wallpaper.png")).ok())
            .map(|image| {
                let index = entries.len();
                entries.push(("wallpaper.png".into(), image));
                index
            });
        let locked = entry_locks(&entries, release, all || unlocked);
        let thumbnails = entries
            .iter()
            .map(|(_, image)| {
                image
                    .resize_to_fill(240, 135, image::imageops::FilterType::Lanczos3)
                    .to_rgba8()
            })
            .collect();
        let saved = store.read().unwrap_or_else(|error| {
            tracing::warn!(%error, "wallpaper selection unavailable");
            None
        });
        let saved_current = saved.as_ref().and_then(|saved| {
            entries
                .iter()
                .position(|(name, _)| name == saved)
                .filter(|index| !locked.get(*index).copied().unwrap_or(true))
        });
        let current = saved_current
            .or(legacy)
            .or_else(|| {
                CODENAMES
                    .get(release)
                    .and_then(|default| entries.iter().position(|(name, _)| name == default))
            })
            .or_else(|| locked.iter().position(|locked| !locked));
        let image = current.and_then(|index| entries.get(index).map(|(_, image)| image.clone()));
        Self {
            locked,
            thumbnails,
            entries,
            current,
            wallpaper: Wallpaper {
                image,
                buffers: std::collections::HashMap::new(),
            },
            store,
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut roots = Vec::new();
        if let Some(data) = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".local/share")))
        {
            roots.push(data.join("tehuti"));
        }
        roots.extend(
            std::env::split_paths(
                &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
            )
            .map(|root| root.join("tehuti")),
        );
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".config")))
            .unwrap_or_else(|| Path::new("/nonexistent").to_path_buf());
        Self::discover(
            &roots,
            crate::display_mode_store::Store::new(config.join("tehuti/wallpaper")),
            CURRENT_RELEASE,
            std::env::var("TEHUTI_ALL_WALLPAPERS").is_ok_and(|value| value == "1"),
        )
    }

    /// # Errors
    /// Returns an error when the permanent unlock cannot be saved.
    pub fn unlock(&mut self) -> std::io::Result<()> {
        self.store.sibling("wallpapers-unlocked").write("1")?;
        self.locked.fill(false);
        Ok(())
    }

    #[must_use]
    pub fn locks(&self) -> Vec<bool> {
        self.locked.clone()
    }

    #[must_use]
    pub fn thumbnails(&self) -> &[image::RgbaImage] {
        &self.thumbnails
    }

    #[must_use]
    pub fn version(&self, index: usize) -> Option<usize> {
        let (name, _) = self.entries.get(index)?;
        CODENAMES.iter().position(|entry| *entry == name)
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|(name, _)| name.clone()).collect()
    }

    /// # Errors
    /// Returns an error when the selection cannot be saved or is unavailable.
    pub fn preview(&mut self, index: usize) -> std::io::Result<()> {
        if self.locked.get(index).copied().unwrap_or(true) {
            return Err(std::io::Error::other("wallpaper locked"));
        }
        let (_, image) = self
            .entries
            .get(index)
            .ok_or_else(|| std::io::Error::other("wallpaper unavailable"))?;
        self.wallpaper.image = Some(image.clone());
        self.wallpaper.buffers.clear();
        Ok(())
    }

    /// # Errors
    /// Returns an error when the wallpaper is locked or saving fails.
    pub fn save(&mut self, index: usize) -> std::io::Result<()> {
        if self.locked.get(index).copied().unwrap_or(true) {
            return Err(std::io::Error::other("wallpaper locked"));
        }
        let name = &self
            .entries
            .get(index)
            .ok_or_else(|| std::io::Error::other("wallpaper unavailable"))?
            .0;
        self.store.write(name)?;
        self.current = Some(index);
        Ok(())
    }

    /// # Errors
    /// Returns an error when the wallpaper is locked or saving fails.
    pub fn select(&mut self, index: usize) -> std::io::Result<()> {
        self.preview(index)?;
        self.save(index)
    }

    /// # Errors
    /// Returns an error when output dimensions are not positive.
    pub fn buffer(&mut self, size: Size<i32, Physical>) -> Result<MemoryRenderBuffer, Box<dyn Error>> {
        self.wallpaper.buffer(size)
    }
}

fn entry_locks(entries: &[(String, image::DynamicImage)], release: usize, unlocked: bool) -> Vec<bool> {
    entries
        .iter()
        .map(|(name, _)| {
            !unlocked
                && CODENAMES
                    .iter()
                    .position(|entry| *entry == name)
                    .is_some_and(|index| index > release)
        })
        .collect()
}

fn load_entries(roots: &[PathBuf], release: usize, all: bool) -> Vec<(String, image::DynamicImage)> {
    let packs: Vec<_> = roots
        .iter()
        .map(|root| {
            let path = root.join("wallpapers.pak");
            match crate::wallpaper_pack::Pack::read(&path) {
                Ok(pack) => Some(pack),
                Err(error) => {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(%error, path = %path.display(), "wallpaper pack unavailable");
                    }
                    None
                }
            }
        })
        .collect();
    CODENAMES
        .iter()
        .enumerate()
        .filter(|(index, _)| all || *index <= release)
        .filter_map(|(_, name)| {
            roots
                .iter()
                .zip(&packs)
                .find_map(|(root, pack)| {
                    pack.as_ref()
                        .and_then(|pack| pack.image(name))
                        .or_else(|| image::open(root.join("wallpapers").join(format!("{name}.png"))).ok())
                })
                .map(|image| ((*name).to_owned(), image))
        })
        .collect()
}

#[derive(Debug)]
pub struct WallpaperCache {
    requests: std::sync::mpsc::SyncSender<Request>,
    redraw: Option<Ping>,
    buffers: std::sync::mpsc::Receiver<(Size<i32, Physical>, u64, Option<MemoryRenderBuffer>)>,
    ready: std::collections::HashMap<(i32, i32), Option<MemoryRenderBuffer>>,
    pending: std::collections::HashSet<(i32, i32)>,
    collection: Option<Collection>,
    generation: u64,
}

impl WallpaperCache {
    #[must_use]
    pub fn new(path: std::path::PathBuf) -> Self {
        let (requests, sizes) = std::sync::mpsc::sync_channel::<Request>(1);
        let (prepared, buffers) = std::sync::mpsc::channel();
        if let Err(error) = std::thread::Builder::new()
            .name("wallpaper".into())
            .spawn(move || {
                let mut wallpaper = Wallpaper::load(&path);
                let mut worker_generation = u64::MAX;
                while let Ok(Request {
                    size,
                    redraw,
                    image,
                    generation,
                }) = sizes.recv()
                {
                    if generation != worker_generation && image.is_some() {
                        wallpaper.image = image;
                        wallpaper.buffers.clear();
                    }
                    worker_generation = generation;
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
                    if prepared.send((size, generation, buffer)).is_err() {
                        break;
                    }
                    if let Some(redraw) = redraw {
                        redraw.ping();
                    }
                }
            })
        {
            tracing::warn!(%error, "wallpaper worker unavailable; using black");
        }
        Self {
            requests,
            redraw: None,
            buffers,
            ready: std::collections::HashMap::new(),
            pending: std::collections::HashSet::new(),
            collection: None,
            generation: 0,
        }
    }

    #[must_use]
    pub fn from_collection(collection: Collection) -> Self {
        let mut cache = Self::new(PathBuf::from("/nonexistent/tehuti/wallpaper.png"));
        cache.collection = Some(collection);
        cache
    }

    #[must_use]
    pub fn choices(&self) -> (Vec<String>, Vec<bool>, usize) {
        self.collection.as_ref().map_or_else(
            || (Vec::new(), Vec::new(), 0),
            |collection| {
                (
                    collection.names(),
                    collection.locks(),
                    collection.current.unwrap_or(0),
                )
            },
        )
    }

    /// # Errors
    /// Returns an error when the unlock marker cannot be written.
    pub fn unlock(&mut self) -> std::io::Result<()> {
        self.collection
            .as_mut()
            .ok_or_else(|| std::io::Error::other("wallpaper collection unavailable"))?
            .unlock()
    }

    /// # Errors
    /// Returns an error when the wallpaper is locked or unavailable.
    pub fn preview(&mut self, index: usize) -> std::io::Result<()> {
        self.collection
            .as_mut()
            .ok_or_else(|| std::io::Error::other("wallpaper collection unavailable"))?
            .preview(index)?;
        self.generation = self.generation.wrapping_add(1);
        self.ready.clear();
        self.pending.clear();
        if let Some(ping) = &self.redraw {
            ping.ping();
        }
        Ok(())
    }

    /// # Errors
    /// Returns an error when the wallpaper is locked or saving fails.
    pub fn save(&mut self, index: usize) -> std::io::Result<()> {
        self.collection
            .as_mut()
            .ok_or_else(|| std::io::Error::other("wallpaper collection unavailable"))?
            .save(index)
    }

    pub fn set_redraw_ping(&mut self, ping: Ping) {
        self.redraw = Some(ping);
    }

    pub fn buffer(&mut self, size: Size<i32, Physical>) -> Option<MemoryRenderBuffer> {
        while let Ok((prepared_size, generation, buffer)) = self.buffers.try_recv() {
            if generation != self.generation {
                continue;
            }
            let _removed = self.pending.remove(&(prepared_size.w, prepared_size.h));
            let _previous = self.ready.insert((prepared_size.w, prepared_size.h), buffer);
        }
        if let Some(buffer) = self.ready.get(&(size.w, size.h)) {
            return buffer.clone();
        }
        if self.pending.insert((size.w, size.h)) {
            match self.requests.try_send(Request {
                size,
                redraw: self.redraw.clone(),
                image: self
                    .collection
                    .as_ref()
                    .map_or_else(|| None, |collection| collection.wallpaper.image.clone()),
                generation: self.generation,
            }) {
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

/// # Errors
/// Returns an error when the wallpaper cache is poisoned.
pub fn set_redraw_ping(ping: Ping) -> Result<(), Box<dyn Error>> {
    wallpaper()
        .lock()
        .map_err(|_| "wallpaper cache unavailable")?
        .set_redraw_ping(ping);
    Ok(())
}

#[must_use]
pub fn choices() -> (Vec<String>, Vec<bool>, usize) {
    wallpaper()
        .lock()
        .map_or_else(|_| (Vec::new(), Vec::new(), 0), |cache| cache.choices())
}

/// # Errors
/// Returns an error when the wallpaper is locked or unavailable.
pub fn preview(index: usize) -> std::io::Result<()> {
    wallpaper()
        .lock()
        .map_err(|_| std::io::Error::other("wallpaper cache unavailable"))?
        .preview(index)
}

/// # Errors
/// Returns an error when the wallpaper is locked or saving fails.
pub fn save(index: usize) -> std::io::Result<()> {
    wallpaper()
        .lock()
        .map_err(|_| std::io::Error::other("wallpaper cache unavailable"))?
        .save(index)
}

/// # Errors
/// Returns an error when the unlock marker cannot be written.
pub fn unlock() -> std::io::Result<(Vec<String>, Vec<bool>, usize)> {
    let mut cache = wallpaper()
        .lock()
        .map_err(|_| std::io::Error::other("wallpaper cache unavailable"))?;
    cache.unlock()?;
    Ok(cache.choices())
}

#[must_use]
pub fn thumbnail(index: usize) -> Option<image::RgbaImage> {
    let cache = wallpaper().lock().ok()?;
    Some(cache.collection.as_ref()?.thumbnails().get(index)?.clone())
}

fn wallpaper() -> &'static Mutex<WallpaperCache> {
    static WALLPAPER: std::sync::OnceLock<Mutex<WallpaperCache>> = std::sync::OnceLock::new();
    WALLPAPER.get_or_init(|| Mutex::new(WallpaperCache::from_collection(Collection::from_env())))
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
    Focus=smithay::backend::renderer::element::solid::SolidColorRenderElement,
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

/// Like [`render_output`], into a buffer that already holds the frame drawn `age` frames ago.
///
/// Age 0 redraws the whole output; otherwise only damage since then is drawn, and an
/// output with no damage draws nothing.
///
/// # Errors
/// Returns an error when window composition or output rendering fails.
pub fn render_output_at_age<'damage, R>(
    output: &Output,
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    compositor: &Compositor,
    overlays: &[MemoryRenderBufferRenderElement<R>],
    damage: &'damage mut OutputDamageTracker,
    age: usize,
) -> Result<RenderOutputResult<'damage>, Box<dyn Error>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: 'static,
{
    render_output_with_cache_at_age(
        output,
        renderer,
        framebuffer,
        compositor,
        overlays,
        damage,
        wallpaper(),
        age,
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
    render_output_with_cache_at_age(
        output,
        renderer,
        framebuffer,
        compositor,
        overlays,
        damage,
        cache,
        0,
    )
}

/// Like [`render_output_with_cache`], into a buffer `age` frames old; see [`render_output_at_age`].
///
/// # Errors
/// Returns an error when window composition or output rendering fails.
#[expect(
    clippy::too_many_arguments,
    reason = "the output, renderer, target, state, overlays, tracker, cache and age are all independent"
)]
pub fn render_output_with_cache_at_age<'damage, R>(
    output: &Output,
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    compositor: &Compositor,
    overlays: &[MemoryRenderBufferRenderElement<R>],
    damage: &'damage mut OutputDamageTracker,
    cache: &Mutex<WallpaperCache>,
    age: usize,
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
    elements.extend(
        compositor
            .focus_outline(output)
            .into_iter()
            .map(OutputRenderElements::Focus),
    );
    elements.extend(space.into_iter().map(OutputRenderElements::Space));
    if let Some(background) = background {
        elements.push(OutputRenderElements::Wallpaper(WallpaperElement(background)));
    }
    Ok(damage.render_output(renderer, framebuffer, age, &elements, [0.0, 0.0, 0.0, 1.0])?)
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
