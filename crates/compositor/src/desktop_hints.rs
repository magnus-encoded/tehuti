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
        .map(|context| context.launch_phrases().to_vec())
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

/// Resolves PNG and optional SVG icons through theme directories, inheritance and unthemed fallbacks.
#[must_use]
pub fn icon_path(name: &str, roots: &[PathBuf], theme: &str) -> Option<PathBuf> {
    icon_path_with_extension(name, roots, theme, "png").or_else(|| {
        cfg!(feature = "svg-icons")
            .then(|| icon_path_with_extension(name, roots, theme, "svg"))
            .flatten()
    })
}

fn icon_path_with_extension(name: &str, roots: &[PathBuf], theme: &str, extension: &str) -> Option<PathBuf> {
    let icon_name = Path::new(name);
    if icon_name.is_absolute() {
        return (icon_name.extension().is_some_and(|value| value == extension) && icon_name.is_file())
            .then(|| icon_name.to_path_buf());
    }
    if name.is_empty() || name.contains('/') || name == ".." {
        return None;
    }
    let filename = if icon_name
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        name.to_owned()
    } else {
        format!("{name}.{extension}")
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
        && let Some(path) = icon_path_with_extension(name, roots, "hicolor", extension)
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

pub(crate) fn load(phrases: &[(String, String)]) -> Vec<Hint> {
    let directories = data_directories();
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".icons"));
    }
    roots.extend(directories.iter().map(|directory| directory.join("icons")));
    roots.extend(directories.iter().map(|directory| directory.join("pixmaps")));
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

pub(crate) fn draw(pixels: Vec<u8>, size: (i32, i32), hints: &[Hint]) -> Option<Vec<u8>> {
    let mut canvas = Canvas::from_pixels(size, pixels)?;
    for (hint, (x, y)) in hints.iter().zip(layout(
        (usize::try_from(size.0).ok()?, usize::try_from(size.1).ok()?),
        hints.len(),
    )) {
        let icon = hint.icon.as_ref().and_then(|path| {
            #[cfg(feature = "svg-icons")]
            if path.extension().is_some_and(|extension| extension == "svg") {
                use std::io::Read;
                let mut bytes = Vec::new();
                let _read = std::fs::File::open(path)
                    .ok()?
                    .take(1_048_577)
                    .read_to_end(&mut bytes)
                    .ok()?;
                return crate::svg_icons::rasterize_svg(&bytes, 48).map(image::DynamicImage::ImageRgba8);
            }
            let mut reader = image::ImageReader::open(path).ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(1024);
            limits.max_image_height = Some(1024);
            limits.max_alloc = Some(8 * 1024 * 1024);
            reader.limits(limits);
            reader.set_format(image::ImageFormat::Png);
            reader.decode().ok()
        });
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
