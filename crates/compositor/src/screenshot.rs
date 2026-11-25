//! Screenshot destination and capture for the system menu.
use crate::state::Compositor;
use core::cell::RefCell;
use core::error::Error;
use std::{path::PathBuf, rc::Rc};

type CaptureFrame = dyn Fn(&Compositor, &str) -> Result<image::RgbaImage, Box<dyn Error>>;

#[derive(Default)]
#[expect(clippy::redundant_pub_crate, reason = "stored in compositor state")]
pub(crate) struct CaptureSource(pub Option<Box<CaptureFrame>>);

impl core::fmt::Debug for CaptureSource {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("CaptureSource").finish_non_exhaustive()
    }
}

impl Compositor {
    /// Uses software capture when the output backend has no GPU renderer.
    pub fn clear_capture_source(&mut self) {
        self.capture_source.0 = None;
    }

    /// Uses the output backend's renderer for screenshots, held weakly across replacement.
    pub fn set_capture_source<D: 'static>(
        &mut self,
        device: &Rc<RefCell<D>>,
        capture: impl Fn(&mut D, &Self, &str) -> Result<image::RgbaImage, Box<dyn Error>> + 'static,
    ) {
        let device = Rc::downgrade(device);
        self.capture_source.0 = Some(Box::new(move |compositor, clock| {
            let device = device.upgrade().ok_or("capture renderer unavailable")?;
            let mut device = device.try_borrow_mut()?;
            capture(&mut device, compositor, clock)
        }));
    }
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture adapters remain private to the compositor"
)]
pub(crate) fn save(compositor: &Compositor) -> Result<PathBuf, Box<dyn Error>> {
    let directory = pictures_directory()?.join("Screenshots");
    std::fs::create_dir_all(&directory)?;
    let now = chrono::Local::now();
    let path = directory.join(format!("Screenshot-{}.png", now.format("%Y-%m-%d_%H-%M-%S-%f")));
    crate::driver_shot::save_with_clock(compositor, &path, &now.format("%H:%M").to_string())?;
    Ok(path)
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture adapters remain private to the compositor"
)]
pub(crate) fn save_window(compositor: &Compositor) -> Result<PathBuf, Box<dyn Error>> {
    let bounds = compositor
        .focused_window_capture_bounds()
        .ok_or("no focused window")?;
    let directory = pictures_directory()?.join("Screenshots");
    std::fs::create_dir_all(&directory)?;
    let now = chrono::Local::now();
    let path = directory.join(format!("Screenshot-{}.png", now.format("%Y-%m-%d_%H-%M-%S-%f")));
    let frame = crate::driver_shot::frame(compositor, &now.format("%H:%M").to_string())?;
    let (left, top, width, height) =
        clamp_crop(bounds, frame.width(), frame.height()).ok_or("focused window is outside the output")?;
    image::imageops::crop_imm(&frame, left, top, width, height)
        .to_image()
        .save_with_format(&path, image::ImageFormat::Png)?;
    Ok(path)
}

fn clamp_crop(
    (x, y, width, height): (i32, i32, i32, i32),
    frame_width: u32,
    frame_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let right = x.saturating_add(width);
    let bottom = y.saturating_add(height);
    let max_x = i32::try_from(frame_width).unwrap_or(i32::MAX);
    let max_y = i32::try_from(frame_height).unwrap_or(i32::MAX);
    let left = x.clamp(0, max_x);
    let top = y.clamp(0, max_y);
    let right = right.clamp(left, max_x);
    let bottom = bottom.clamp(top, max_y);
    let cropped_width = u32::try_from(right.saturating_sub(left)).ok()?;
    let cropped_height = u32::try_from(bottom.saturating_sub(top)).ok()?;
    (cropped_width > 0 && cropped_height > 0).then(|| {
        (
            u32::try_from(left).unwrap_or_default(),
            u32::try_from(top).unwrap_or_default(),
            cropped_width,
            cropped_height,
        )
    })
}

fn pictures_directory() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = std::env::var_os("XDG_PICTURES_DIR").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err("XDG_PICTURES_DIR must be absolute".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unavailable")?);
    let config = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    match std::fs::read_to_string(config.join("user-dirs.dirs")) {
        Ok(text) => {
            for line in text.lines() {
                if let Some(value) = line
                    .trim()
                    .strip_prefix("XDG_PICTURES_DIR=")
                    .and_then(|value| value.strip_prefix('"'))
                    .and_then(|value| value.strip_suffix('"'))
                {
                    let path = value
                        .strip_prefix("$HOME/")
                        .map_or_else(|| PathBuf::from(value), |relative| home.join(relative));
                    if path.is_absolute() {
                        return Ok(path);
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(home.join("Pictures"))
}
