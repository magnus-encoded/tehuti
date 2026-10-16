//! Couch-readable settings artwork.

use core::time::Duration;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use tehuti_core::dispatch::{Settings, SettingsView};

#[derive(Debug, Default)]
pub(crate) struct Display {
    pub requests: std::collections::VecDeque<(usize, bool)>,
    pub artwork: core::cell::RefCell<Artwork>,
}

#[derive(Debug, Default)]
pub struct Artwork {
    cached: Option<(Settings, (i32, i32), MemoryRenderBuffer)>,
}

impl Artwork {
    pub fn buffer(&mut self, settings: &Settings, size: (i32, i32)) -> Option<MemoryRenderBuffer> {
        self.cached_buffer(settings, size, None)
    }

    pub fn buffer_with_wallpapers(
        &mut self,
        settings: &Settings,
        size: (i32, i32),
        wallpapers: &crate::wallpaper::Collection,
    ) -> Option<MemoryRenderBuffer> {
        self.cached_buffer(settings, size, Some(wallpapers))
    }

    fn cached_buffer(
        &mut self,
        settings: &Settings,
        size: (i32, i32),
        wallpapers: Option<&crate::wallpaper::Collection>,
    ) -> Option<MemoryRenderBuffer> {
        if !settings.open {
            self.cached = None;
            return None;
        }
        let mut key = settings.clone();
        key.remaining = key.remaining.map(|remaining| {
            Duration::from_millis(
                u64::try_from(remaining.as_millis() / 100)
                    .unwrap_or_default()
                    .saturating_mul(100),
            )
        });
        if self
            .cached
            .as_ref()
            .is_none_or(|(previous, previous_size, _)| *previous != key || *previous_size != size)
        {
            self.cached = draw_buffer(&key, size, wallpapers).map(|buffer| (key, size, buffer));
        }
        self.cached.as_ref().map(|(_, _, buffer)| buffer.clone())
    }
}

/// # Errors
/// Returns the renderer's memory import error.
pub fn element<R>(
    renderer: &mut R,
    compositor: &crate::state::Compositor,
    output: &smithay::output::Output,
) -> Result<Option<smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>>, R::Error>
where
    R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let Some(mode) = output.current_mode() else {
        return Ok(None);
    };
    let Some(buffer) = compositor.settings_buffer((mode.size.w, mode.size.h)) else {
        return Ok(None);
    };
    smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        &buffer,
        None,
        Some(smithay::utils::Rectangle::from_size(
            (f64::from(mode.size.w), f64::from(mode.size.h)).into(),
        )),
        Some(crate::scale::logical_size(
            mode.size,
            output.current_scale().integer_scale(),
        )),
        smithay::backend::renderer::element::Kind::Unspecified,
    )
    .map(Some)
}

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const CYAN: [u8; 4] = [240, 218, 64, 255];
const AMBER: [u8; 4] = [56, 184, 255, 255];
const WHITE: [u8; 4] = [244, 240, 225, 255];

pub(crate) mod canvas;
use canvas::Canvas;

impl Canvas {
    fn contents(&mut self, settings: &Settings, wallpapers: Option<&crate::wallpaper::Collection>) {
        if settings.view == SettingsView::Wallpaper {
            let first = settings
                .selected
                .saturating_div(4)
                .saturating_sub(1)
                .saturating_mul(4);
            for index in first..settings.wallpapers.len().min(first.saturating_add(8)) {
                let column = index.saturating_sub(first).checked_rem(4).unwrap_or_default();
                let row = index.saturating_sub(first).saturating_div(4);
                let x = 80usize.saturating_add(column.saturating_mul(280));
                let y = 244usize.saturating_add(row.saturating_mul(174));
                let locked = settings.wallpaper_locked.get(index).copied().unwrap_or(false);
                self.rectangle(
                    x.saturating_sub(4),
                    y.saturating_sub(4),
                    248,
                    143,
                    if index == settings.selected {
                        CYAN
                    } else {
                        [65, 43, 12, 255]
                    },
                );
                if let Some(image) = wallpapers.and_then(|collection| collection.thumbnails().get(index)) {
                    self.thumbnail(x, y, image, locked);
                }
                if locked {
                    let version = wallpapers
                        .and_then(|collection| collection.version(index))
                        .unwrap_or(index);
                    self.text(
                        x.saturating_add(8),
                        y.saturating_add(96),
                        &format!("v{version}"),
                        3,
                        AMBER,
                    );
                } else if Some(index) == settings.wallpaper_current {
                    self.text(x.saturating_add(8), y.saturating_add(104), "ACTIVE", 2, CYAN);
                }
            }
            if settings.wallpapers.is_empty() {
                self.text(96, 256, "NO WALLPAPERS AVAILABLE", 3, AMBER);
            }
        } else if settings.view == SettingsView::Display {
            self.display_choices(settings);
        } else {
            self.sections(settings);
        }
    }

    fn display_choices(&mut self, settings: &Settings) {
        self.text(
            96,
            230,
            "SIZE",
            3,
            if settings.frequency_group { WHITE } else { CYAN },
        );
        self.text(
            672,
            230,
            "FREQUENCY",
            3,
            if settings.frequency_group { CYAN } else { WHITE },
        );
        self.size_choices(settings);
        self.frequency_choices(settings);
        if settings.modes.is_empty() {
            self.text(96, 300, "NO DRIVER MODES AVAILABLE", 3, AMBER);
        }
    }

    fn size_choices(&mut self, settings: &Settings) {
        let sizes = settings.sizes();
        let native = settings.native_size.unwrap_or_else(|| {
            sizes
                .iter()
                .copied()
                .max_by_key(|size| u64::from(size.0).saturating_mul(u64::from(size.1)))
                .unwrap_or((1, 1))
        });
        let frame = screen_frame(native);
        let selected_size = sizes
            .iter()
            .position(|size| {
                settings
                    .modes
                    .get(settings.selected)
                    .is_some_and(|mode| mode.starts_with(&format!("{}x{}@", size.0, size.1)))
            })
            .unwrap_or_default();
        let first = selected_size.saturating_sub(2);
        for (row, (index, size)) in sizes.iter().enumerate().skip(first).take(3).enumerate() {
            let y = 274usize.saturating_add(row.saturating_mul(100));
            if index == selected_size {
                self.rectangle(80, y.saturating_sub(6), 530, 94, [65, 43, 12, 255]);
                self.rectangle(
                    80,
                    y.saturating_sub(6),
                    4,
                    94,
                    if settings.frequency_group { WHITE } else { CYAN },
                );
            }
            let (width, height) = size_rectangle(*size, native, frame);
            self.rectangle(
                100usize.saturating_add(frame.0.saturating_sub(width) / 2),
                y.saturating_add(frame.1.saturating_sub(height) / 2),
                width,
                height,
                CYAN,
            );
            self.outline(100, y, frame.0, frame.1, [100, 90, 70, 255]);
            self.text(
                270,
                y.saturating_add(24),
                &format!("{}×{}", size.0, size.1),
                3,
                WHITE,
            );
        }
    }

    fn frequency_choices(&mut self, settings: &Settings) {
        let rates = settings.rates();
        let selected_rate = rates
            .iter()
            .position(|index| *index == settings.selected)
            .unwrap_or_default();
        for (row, index) in rates
            .iter()
            .skip(selected_rate.saturating_sub(2))
            .take(5)
            .enumerate()
        {
            let y = 278usize.saturating_add(row.saturating_mul(54));
            if *index == settings.selected {
                self.rectangle(656, y.saturating_sub(8), 326, 50, [65, 43, 12, 255]);
                self.rectangle(
                    656,
                    y.saturating_sub(8),
                    4,
                    50,
                    if settings.frequency_group { CYAN } else { WHITE },
                );
            }
            if let Some((_, rate)) = settings.modes.get(*index).and_then(|mode| mode.split_once('@')) {
                let hertz = rate.parse::<f32>().unwrap_or(60.0);
                self.wave(676, y.saturating_add(14), hertz);
                self.text(848, y, &format!("{rate} HZ"), 3, WHITE);
            }
        }
    }

    fn outline(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
        self.rectangle(x, y, width, 2, color);
        self.rectangle(x, y.saturating_add(height.saturating_sub(2)), width, 2, color);
        self.rectangle(x, y, 2, height, color);
        self.rectangle(x.saturating_add(width.saturating_sub(2)), y, 2, height, color);
    }

    fn sections(&mut self, settings: &Settings) {
        for (index, (title, detail)) in [
            ("DISPLAY", "RESOLUTION / REFRESH"),
            ("WALLPAPER", "RELEASE COLLECTION"),
        ]
        .iter()
        .enumerate()
        {
            let y = 236usize.saturating_add(index.saturating_mul(100));
            if settings.section == index {
                self.rectangle(72, y, 1128, 76, [65, 43, 12, 255]);
                self.rectangle(72, y, 4, 76, CYAN);
            }
            self.text(96, y.saturating_add(20), title, 5, WHITE);
            self.text(600, y.saturating_add(32), detail, 3, CYAN);
        }
    }

    fn ring(&mut self, remaining: Duration) {
        self.countdown(remaining);
        self.text(
            1060,
            535,
            &remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() > 0))
                .to_string(),
            5,
            AMBER,
        );
    }
}

#[must_use]
pub fn buffer(settings: &Settings, size: (i32, i32)) -> Option<MemoryRenderBuffer> {
    draw_buffer(settings, size, None)
}

fn draw_buffer(
    settings: &Settings,
    size: (i32, i32),
    wallpapers: Option<&crate::wallpaper::Collection>,
) -> Option<MemoryRenderBuffer> {
    if !settings.open {
        return None;
    }
    let mut canvas = Canvas::new(size)?;
    canvas.rectangle(0, 0, WIDTH, HEIGHT, [20, 14, 8, 235]);
    canvas.rectangle(48, 52, 6, 80, CYAN);
    canvas.text(80, 54, "TEHUTI / SYSTEM", 3, CYAN);
    canvas.text(
        80,
        92,
        if settings.view == SettingsView::Wallpaper {
            "WALLPAPER"
        } else if settings.view == SettingsView::Display {
            "DISPLAY"
        } else {
            "SETTINGS"
        },
        6,
        WHITE,
    );
    canvas.rectangle(80, 166, 1120, 2, CYAN);
    canvas.text(80, 190, "POP THE HOOD", 2, AMBER);
    canvas.contents(settings, wallpapers);
    if let Some(remaining) = settings.remaining {
        canvas.ring(remaining);
        canvas.text(80, 604, settings.button_hint(), 3, AMBER);
        canvas.text(80, 642, "AUTO REVERT IN 15 SECONDS", 2, WHITE);
    } else {
        if settings.failed() {
            canvas.text(80, 606, "MODE REJECTED / PREVIOUS MODE RESTORED", 2, AMBER);
        }
        canvas.text(80, 656, settings.button_hint(), 3, CYAN);
    }
    Some(MemoryRenderBuffer::from_slice(
        &canvas.pixels,
        Fourcc::Argb8888,
        size,
        1,
        Transform::Normal,
        None,
    ))
}

#[cfg(test)]
mod tests {
    use crate::state::Compositor;
    use smithay::{
        backend::renderer::{element::Element, pixman::PixmanRenderer},
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        reexports::wayland_server::Display,
        utils::{Rectangle, Transform},
    };
    use tehuti_protocol::{Event, Text};

    #[test]
    fn native_artwork_has_smooth_text_and_countdown_edges() -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.configure_display_modes(vec!["1920x1080@60".into(), "3840x2160@60".into()], 0);
        compositor.handle(&Event::Speech {
            text: Text::new("Pop the hood.".into())?,
        });
        for name in [
            tehuti_protocol::ButtonName::South,
            tehuti_protocol::ButtonName::DpadDown,
            tehuti_protocol::ButtonName::South,
        ] {
            compositor.handle(&Event::Button {
                name,
                state: tehuti_protocol::ButtonState::Pressed,
            });
        }
        compositor.display_mode_applied(Some(1));
        let mut artwork = compositor
            .settings_buffer((1920, 1080))
            .ok_or("artwork unavailable")?;
        artwork.render().draw(|pixels| {
            let text_colors: std::collections::HashSet<_> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    (138..210).contains(&index.saturating_div(1920))
                        && (120..650).contains(&index.checked_rem(1920).unwrap_or_default())
                })
                .map(|(_, pixel)| pixel.to_vec())
                .collect();
            assert!(text_colors.len() > 30, "font edges have fractional coverage");
            let ring_colors: std::collections::HashSet<_> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    (690..960).contains(&index.saturating_div(1920))
                        && (1500..1770).contains(&index.checked_rem(1920).unwrap_or_default())
                })
                .map(|(_, pixel)| pixel.to_vec())
                .collect();
            assert!(ring_colors.len() > 30, "countdown edges have fractional coverage");
            Ok::<_, Box<dyn core::error::Error>>(Vec::new())
        })?;
        Ok(())
    }

    #[test]
    fn settings_fill_the_output_at_each_integer_scale() -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.handle(&Event::Speech {
            text: Text::new("Pop the hood.".into())?,
        });
        let output = Output::new(
            "settings-test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        let mut renderer = PixmanRenderer::new()?;
        for (size, scale) in [((1280, 720), 1), ((1920, 1080), 1), ((3840, 2160), 2)] {
            output.change_current_state(
                Some(Mode {
                    size: size.into(),
                    refresh: 60_000,
                }),
                Some(Transform::Normal),
                Some(Scale::Integer(scale)),
                Some((0, 0).into()),
            );
            let element =
                super::element(&mut renderer, &compositor, &output)?.ok_or("settings element unavailable")?;
            assert_eq!(
                element.geometry(f64::from(scale).into()),
                Rectangle::from_size(size.into())
            );
            assert_eq!(element.src().size, (f64::from(size.0), f64::from(size.1)).into());
        }
        Ok(())
    }
}

#[must_use]
pub fn size_rectangle(size: (u32, u32), native: (u32, u32), frame: (usize, usize)) -> (usize, usize) {
    let dimension = |value: u32, maximum: u32, extent: usize| {
        usize::try_from(
            u64::from(value.min(maximum))
                .saturating_mul(u64::try_from(extent).unwrap_or_default())
                .checked_div(u64::from(maximum))
                .unwrap_or_default(),
        )
        .unwrap_or_default()
    };
    (
        dimension(size.0, native.0, frame.0),
        dimension(size.1, native.1, frame.1),
    )
}

#[cfg(test)]
mod geometry_tests {
    #[test]
    fn locked_thumbnail_is_dimmed_and_unlock_restores_cached_color() -> Result<(), Box<dyn core::error::Error>>
    {
        use tehuti_core::dispatch::{Settings, SettingsView};
        let directory = tempfile::tempdir()?;
        std::fs::create_dir(directory.path().join("wallpapers"))?;
        image::RgbaImage::from_pixel(2, 2, image::Rgba([90, 120, 150, 255]))
            .save(directory.path().join("wallpapers/bostrychia.png"))?;
        let mut collection = crate::wallpaper::Collection::discover(
            &[directory.path().to_path_buf()],
            crate::display_mode_store::Store::new(directory.path().join("selection")),
            0,
            false,
        );
        let mut settings = Settings::default();
        settings.open = true;
        settings.view = SettingsView::Wallpaper;
        settings.wallpapers = collection.names();
        settings.wallpaper_locked = collection.locks();
        let mut artwork = super::Artwork::default();
        let sample = |mut buffer: smithay::backend::renderer::element::memory::MemoryRenderBuffer| -> Result<Vec<u8>, Box<dyn core::error::Error>> {
            let mut pixel = Vec::new();
            buffer.render().draw(|pixels| {
                pixel = pixels.get(333_000..333_004).ok_or("thumbnail pixel unavailable")?.to_vec();
                Ok::<_, Box<dyn core::error::Error>>(Vec::new())
            })?;
            Ok(pixel)
        };
        let locked = artwork
            .buffer_with_wallpapers(&settings, (640, 360), &collection)
            .ok_or("artwork unavailable")?;
        assert_eq!(sample(locked)?, [50, 40, 30, 255]);
        collection.unlock()?;
        settings.wallpaper_locked = collection.locks();
        let unlocked = artwork
            .buffer_with_wallpapers(&settings, (640, 360), &collection)
            .ok_or("artwork unavailable")?;
        assert_eq!(sample(unlocked)?, [150, 120, 90, 255]);
        Ok(())
    }

    #[test]
    fn size_rectangles_keep_each_dimension_relative_to_native() {
        assert_eq!(
            super::size_rectangle((3840, 2160), (3840, 2160), (160, 90)),
            (160, 90)
        );
        assert_eq!(
            super::size_rectangle((1920, 1080), (3840, 2160), (160, 90)),
            (80, 45)
        );
        assert_eq!(
            super::size_rectangle((1280, 1024), (2560, 1440), (180, 90)),
            (90, 64)
        );
        assert_eq!(
            super::size_rectangle((4096, 2160), (3840, 2160), (160, 90)),
            (160, 90)
        );
        assert_eq!(super::size_rectangle((1920, 1080), (0, 0), (160, 90)), (0, 0));
    }
}

fn screen_frame(native: (u32, u32)) -> (usize, usize) {
    let ratio = |value: u32, extent: u64, divisor: u32| {
        usize::try_from(
            u64::from(value)
                .saturating_mul(extent)
                .checked_div(u64::from(divisor))
                .unwrap_or_default(),
        )
        .unwrap_or_default()
    };
    if u64::from(native.0).saturating_mul(78) > u64::from(native.1).saturating_mul(144) {
        (144, ratio(native.1, 144, native.0))
    } else {
        (ratio(native.0, 78, native.1), 78)
    }
}
