//! Desktop entry and icon lookup for launcher rows.

use std::path::{Path, PathBuf};

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
