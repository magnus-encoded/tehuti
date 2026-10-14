//! Effects of the system menu: spawned commands and screenshot files.
//!
//! The core decides; this module only carries out. Tests swap the
//! [`Executor`] for a fake so nothing real ever runs.

use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Runs system programs on behalf of the menu.
pub trait Executor: core::fmt::Debug {
    /// Starts `program` without waiting for it.
    ///
    /// # Errors
    /// The program could not be started.
    fn spawn(&mut self, program: &str, args: &[&str]) -> io::Result<()>;

    /// The screen locker to start, when one is configured.
    fn locker(&self) -> Option<String>;
}

/// The real executor: spawns children without a shell or privilege escalation.
#[derive(Debug, Default)]
pub struct Real;

impl Executor for Real {
    fn spawn(&mut self, program: &str, args: &[&str]) -> io::Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let _reaper = std::thread::spawn(move || child.wait());
        Ok(())
    }

    /// `TEHUTI_LOCKER` names a program; no default exists because Tehuti has no session-lock protocol yet.
    fn locker(&self) -> Option<String> {
        std::env::var("TEHUTI_LOCKER")
            .ok()
            .filter(|name| !name.is_empty())
    }
}

/// `YYYYMMDD-HHMMSS` in UTC for a Unix time.
#[must_use]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "civil-calendar arithmetic on day counts far below i64 limits; tests pin known dates"
)]
pub fn timestamp(unix_seconds: u64) -> String {
    let days = unix_seconds / 86_400;
    let rest = unix_seconds % 86_400;
    // Civil-from-days (Howard Hinnant), valid for the whole u64 range we can meet.
    let shifted = i64::try_from(days).unwrap_or(0).saturating_add(719_468);
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// `<home>/Pictures/Screenshots/tehuti-<timestamp>.png`.
#[must_use]
pub fn screenshot_path(home: &Path, unix_seconds: u64) -> PathBuf {
    home.join("Pictures")
        .join("Screenshots")
        .join(format!("tehuti-{}.png", timestamp(unix_seconds)))
}

/// Writes RGBA pixels as a PNG, creating the directory first.
///
/// # Errors
/// The directory or file could not be written, or the pixel count does not match the size.
pub fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    let expected = usize::try_from(
        u64::from(width)
            .saturating_mul(u64::from(height))
            .saturating_mul(4),
    )
    .map_err(|_| io::Error::other("screenshot too large"))?;
    if rgba.len() != expected || expected == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pixel count does not match size",
        ));
    }
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(rgba).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)
}

/// Turns a captured frame into opaque RGBA rows, top row first.
#[must_use]
pub fn normalize(mut pixels: Vec<u8>, width: usize, flipped: bool) -> Vec<u8> {
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    let stride = width.saturating_mul(4);
    if flipped && stride > 0 {
        return pixels.chunks_exact(stride).rev().flatten().copied().collect();
    }
    pixels
}

/// Reads the bound framebuffer back and saves it as a PNG.
///
/// # Errors
/// The framebuffer could not be read or the file could not be written.
pub fn capture<R>(
    renderer: &mut R,
    framebuffer: &<R as smithay::backend::renderer::RendererSuper>::Framebuffer<'_>,
    size: (i32, i32),
    path: &Path,
) -> io::Result<()>
where
    R: smithay::backend::renderer::ExportMem,
{
    use smithay::backend::renderer::TextureMapping as _;
    let region = smithay::utils::Rectangle::<i32, smithay::utils::Buffer>::from_size(size.into());
    let mapping = renderer
        .copy_framebuffer(framebuffer, region, smithay::backend::allocator::Fourcc::Abgr8888)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let flipped = mapping.flipped();
    let pixels = renderer
        .map_texture(&mapping)
        .map_err(|error| io::Error::other(error.to_string()))?
        .to_vec();
    let width = u32::try_from(size.0).map_err(io::Error::other)?;
    let height = u32::try_from(size.1).map_err(io::Error::other)?;
    save_png(
        path,
        width,
        height,
        &normalize(pixels, usize::try_from(width).unwrap_or(0), flipped),
    )
}
