//! Voice launch hints prepared with wallpaper artwork.

use std::path::{Path, PathBuf};

use crate::settings::canvas::Canvas;

/// A desktop entry and its first configured launch phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub caption: String,
    pub name: String,
    pub icon: Option<PathBuf>,
}

/// Launch phrases from the first global context, matching dispatch priority.
#[must_use]
pub fn launch_phrases(contexts: &[tehuti_protocol::Context]) -> Vec<(String, String)> {
    contexts
        .iter()
        .find(|context| context.app_id() == "*")
        .map(|context| {
            let mut phrases = context.launch_phrases().to_vec();
            phrases.sort_by_key(|(desktop_entry, _)| {
                context
                    .favourites()
                    .iter()
                    .position(|entry| entry == desktop_entry)
                    .unwrap_or(usize::MAX)
            });
            phrases
        })
        .unwrap_or_default()
}

/// Reads the main desktop entry group, ignoring localized keys and action groups.
#[must_use]
pub fn entry_metadata(source: &str) -> Option<(String, String)> {
    let mut main = false;
    let mut name = None;
    let mut icon = String::new();
    for line in source.lines().map(str::trim) {
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
        } else if main
            && !line.starts_with('#')
            && let Some((key, value)) = line.split_once('=')
        {
            match key.trim() {
                "Name" => name = Some(value.trim().to_owned()),
                "Icon" => value.trim().clone_into(&mut icon),
                _ => {}
            }
        }
    }
    name.filter(|name| !name.is_empty()).map(|name| (name, icon))
}

fn data_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME").filter(|home| !home.is_empty()) {
        directories.push(PathBuf::from(home));
    } else if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share"));
    }
    directories.extend(std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ));
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/share/flatpak/exports/share"));
    }
    directories.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    directories
}

fn desktop_path(directory: &Path, entry_id: &str, depth: usize) -> Option<PathBuf> {
    if depth >= 16 {
        return None;
    }
    let children = std::fs::read_dir(directory).ok()?;
    for child in children.flatten() {
        let name = child.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == entry_id && child.path().is_file() {
            return Some(child.path());
        }
        if child.file_type().is_ok_and(|kind| kind.is_dir())
            && let Some(rest) = entry_id.strip_prefix(&format!("{name}-"))
            && let Some(path) = desktop_path(&child.path(), rest, depth.saturating_add(1))
        {
            return Some(path);
        }
    }
    None
}

/// Resolves PNG, then SVG, icons through theme directories, inheritance and unthemed fallbacks.
#[must_use]
pub fn icon_path(name: &str, roots: &[PathBuf], theme: &str) -> Option<PathBuf> {
    icon_with_extension(name, roots, theme, "png").or_else(|| icon_with_extension(name, roots, theme, "svg"))
}

fn icon_with_extension(name: &str, roots: &[PathBuf], theme: &str, wanted: &str) -> Option<PathBuf> {
    let icon_name = Path::new(name);
    let has_wanted = icon_name
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted));
    if icon_name.is_absolute() {
        return (has_wanted && icon_name.is_file()).then(|| icon_name.to_path_buf());
    }
    if name.is_empty() || name.contains('/') || name == ".." {
        return None;
    }
    let filename = if has_wanted {
        name.to_owned()
    } else {
        format!("{name}.{wanted}")
    };
    let mut themes = vec![theme.to_owned()];
    let mut visited = std::collections::HashSet::new();
    while let Some(current_theme) = themes.pop() {
        if !visited.insert(current_theme.clone())
            || visited.len() > 16
            || current_theme.contains('/')
            || current_theme == ".."
        {
            continue;
        }
        for root in roots {
            let base = root.join(&current_theme);
            let Ok(index) = std::fs::read_to_string(base.join("index.theme")) else {
                continue;
            };
            if let Some(found) = theme_icon(&index, &base, &filename, &mut themes) {
                return Some(found);
            }
        }
    }
    if theme != "hicolor"
        && let Some(path) = icon_with_extension(name, roots, "hicolor", wanted)
    {
        return Some(path);
    }
    roots
        .iter()
        .map(|root| root.join(&filename))
        .find(|path| path.is_file())
}

fn theme_icon(index: &str, base: &Path, filename: &str, themes: &mut Vec<String>) -> Option<PathBuf> {
    let mut group = false;
    for line in index.lines().map(str::trim) {
        if line.starts_with('[') {
            group = line == "[Icon Theme]";
        } else if group {
            if let Some(inherits) = line.strip_prefix("Inherits=") {
                themes.extend(inherits.split(',').rev().map(str::trim).map(str::to_owned));
            }
            if let Some(directories) = line
                .strip_prefix("Directories=")
                .or_else(|| line.strip_prefix("ScaledDirectories="))
            {
                for directory in directories.split(',').map(str::trim) {
                    if Path::new(directory)
                        .components()
                        .any(|part| !matches!(part, std::path::Component::Normal(_)))
                    {
                        continue;
                    }
                    let candidate = base.join(directory).join(filename);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    None
}

/// Resolves an icon using the desktop's data directories and theme.
#[must_use]
pub fn resolve_icon(name: &str) -> Option<PathBuf> {
    icon_path(name, &icon_roots(&data_directories()), "hicolor")
}

fn icon_roots(directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".icons"));
    }
    roots.extend(directories.iter().map(|directory| directory.join("icons")));
    roots.extend(directories.iter().map(|directory| directory.join("pixmaps")));
    roots
}

/// Decodes desktop artwork within the shell's image limits.
#[must_use]
pub fn read_icon(path: &Path) -> Option<image::DynamicImage> {
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return rasterise_svg(path);
    }
    let mut reader = image::ImageReader::open(path).ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    reader.set_format(image::ImageFormat::Png);
    reader.decode().ok()
}

/// Decodes icons on a worker thread so a slow or hostile file never stalls a frame.
/// Each message is `(index, decoded)`; a file that fails or never finishes leaves
/// its slot on the neutral icon.
#[must_use]
pub fn spawn_icon_loader(
    paths: Vec<Option<PathBuf>>,
) -> std::sync::mpsc::Receiver<(usize, Option<image::DynamicImage>)> {
    let (sender, receiver) = std::sync::mpsc::channel();
    for (index, path) in paths.into_iter().enumerate() {
        let Some(path) = path else { continue };
        let sender = sender.clone();
        // Detached on purpose: a stuck decode only strands its own thread.
        drop(std::thread::spawn(move || {
            drop(sender.send((index, read_icon(&path))));
        }));
    }
    receiver
}

/// Rasterises vector artwork to a square RGBA image sized for launcher rows.
#[expect(
    clippy::float_arithmetic,
    reason = "vector scaling is inherently floating point"
)]
fn rasterise_svg(path: &Path) -> Option<image::DynamicImage> {
    const SIZE: u32 = 96;
    const SIDE: f32 = 96.0;
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    if std::fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = f32::min(SIDE / size.width(), SIDE / size.height());
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(SIZE, SIZE)?;
    let offset_x = size.width().mul_add(-scale, SIDE) / 2.0;
    let offset_y = size.height().mul_add(-scale, SIDE) / 2.0;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y),
        &mut pixmap.as_mut(),
    );
    let straight: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let colour = pixel.demultiply();
            [colour.red(), colour.green(), colour.blue(), colour.alpha()]
        })
        .collect();
    image::RgbaImage::from_raw(SIZE, SIZE, straight).map(image::DynamicImage::ImageRgba8)
}

pub(crate) fn load(phrases: &[(String, String)]) -> Vec<Hint> {
    let directories = data_directories();
    let roots = icon_roots(&directories);
    phrases
        .iter()
        .filter_map(|(entry_id, caption)| {
            let entry_id = if Path::new(entry_id)
                .extension()
                .is_some_and(|extension| extension == "desktop")
            {
                entry_id.clone()
            } else {
                format!("{entry_id}.desktop")
            };
            let path = directories
                .iter()
                .find_map(|directory| desktop_path(&directory.join("applications"), &entry_id, 0))?;
            let source = std::fs::read_to_string(path).ok()?;
            let (name, icon) = entry_metadata(&source)?;
            Some(Hint {
                caption: caption.clone(),
                name,
                icon: icon_path(&icon, &roots, "hicolor"),
            })
        })
        .collect()
}

/// Visible cells in row order, with space reserved for the bar.
#[must_use]
pub fn layout(size: (usize, usize), count: usize) -> Vec<(usize, usize)> {
    let columns = size.0.saturating_sub(32) / 160;
    let rows = size.1.saturating_sub(80) / 112;
    (0..count.min(columns.saturating_mul(rows)))
        .map(|index| {
            (
                32_usize.saturating_add(index.checked_rem(columns).unwrap_or(0).saturating_mul(160)),
                48_usize.saturating_add(index.checked_div(columns).unwrap_or(0).saturating_mul(112)),
            )
        })
        .collect()
}

pub(crate) fn draw(
    pixels: Vec<u8>,
    size: (i32, i32),
    hints: &[Hint],
    focus: Option<usize>,
) -> Option<Vec<u8>> {
    let mut canvas = Canvas::from_pixels(size, pixels)?;
    for (index, (hint, (x, y))) in hints
        .iter()
        .zip(layout(
            (usize::try_from(size.0).ok()?, usize::try_from(size.1).ok()?),
            hints.len(),
        ))
        .enumerate()
    {
        if focus == Some(index) {
            canvas.rectangle(
                x.saturating_sub(3),
                y.saturating_sub(3),
                158,
                106,
                [255, 208, 64, 255],
            );
        }
        let icon = hint.icon.as_ref().and_then(|path| read_icon(path));
        if let Some(icon) = icon {
            canvas.image(x, y, &icon.thumbnail(48, 48).to_rgba8());
        } else {
            canvas.rectangle(x, y, 48, 48, [40, 92, 116, 255]);
            canvas.text(
                x.saturating_add(12),
                y.saturating_add(8),
                &hint.name.chars().take(1).collect::<String>(),
                4,
                [255; 4],
            );
        }
        canvas.rectangle(x, y.saturating_add(52), 152, 48, [0, 0, 0, 210]);
        canvas.text_box(
            (x.saturating_add(4), y.saturating_add(54), 144, 44),
            &hint.caption,
            2,
            [255; 4],
        );
    }
    Some(canvas.pixels)
}
