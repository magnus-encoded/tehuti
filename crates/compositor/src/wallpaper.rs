//! File wallpaper, cached at each output size.

use core::error::Error;
use std::path::{Path, PathBuf};

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

pub(crate) const CODENAMES: &[&str] = &[
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
        let entries = load_entries(roots, release, true);
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
        let legacy = saved_current
            .is_none()
            .then(|| {
                roots
                    .iter()
                    .find_map(|root| image::open(root.join("wallpaper.png")).ok())
            })
            .flatten();
        let current = saved_current
            .or_else(|| {
                CODENAMES
                    .get(release)
                    .and_then(|default| entries.iter().position(|(name, _)| name == default))
            })
            .or_else(|| locked.iter().position(|locked| !locked));
        let current = current.filter(|_| legacy.is_none());
        let image =
            legacy.or_else(|| current.and_then(|index| entries.get(index).map(|(_, image)| image.clone())));
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
    pub fn select(&mut self, index: usize) -> std::io::Result<()> {
        if self.locked.get(index).copied().unwrap_or(true) {
            return Err(std::io::Error::other("wallpaper locked"));
        }
        let (name, image) = self
            .entries
            .get(index)
            .ok_or_else(|| std::io::Error::other("wallpaper unavailable"))?;
        self.current = Some(index);
        self.wallpaper.image = Some(image.clone());
        self.wallpaper.buffers.clear();
        self.store.write(name)
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
    let buffer = compositor.wallpapers.borrow_mut().buffer(size)?;
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
