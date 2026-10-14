//! Couch-readable settings artwork.

use core::time::Duration;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use tehuti_core::dispatch::Settings;

#[derive(Debug, Default)]
pub(crate) struct Display {
    pub requests: std::collections::VecDeque<(usize, bool)>,
    pub artwork: core::cell::RefCell<Artwork>,
}

#[derive(Debug, Default)]
pub struct Artwork {
    cached: Option<(Settings, MemoryRenderBuffer)>,
}

impl Artwork {
    pub fn buffer(&mut self, settings: &Settings) -> Option<MemoryRenderBuffer> {
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
        if self.cached.as_ref().is_none_or(|(previous, _)| *previous != key) {
            self.cached = buffer(&key).map(|buffer| (key, buffer));
        }
        self.cached.as_ref().map(|(_, buffer)| buffer.clone())
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
    let Some(buffer) = compositor.settings_buffer() else {
        return Ok(None);
    };
    let Some(mode) = output.current_mode() else {
        return Ok(None);
    };
    smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        &buffer,
        None,
        Some(smithay::utils::Rectangle::from_size((1280.0, 720.0).into())),
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

struct Canvas(Vec<u8>);

impl Canvas {
    fn rectangle(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
        for row in y..y.saturating_add(height).min(HEIGHT) {
            for column in x..x.saturating_add(width).min(WIDTH) {
                let offset = row.saturating_mul(WIDTH).saturating_add(column).saturating_mul(4);
                if let Some(pixel) = self.0.get_mut(offset..offset.saturating_add(4)) {
                    pixel.copy_from_slice(&color);
                }
            }
        }
    }

    fn text(&mut self, x: usize, y: usize, text: &str, scale: usize, color: [u8; 4]) {
        for (letter, character) in text.chars().enumerate() {
            for (row, bits) in crate::notice::glyph(character.to_ascii_uppercase())
                .iter()
                .enumerate()
            {
                for column in 0..5usize {
                    if bits
                        & (16_u8
                            .checked_shr(u32::try_from(column).unwrap_or_default())
                            .unwrap_or_default())
                        != 0
                    {
                        self.rectangle(
                            x.saturating_add(letter.saturating_mul(6).saturating_mul(scale))
                                .saturating_add(column.saturating_mul(scale)),
                            y.saturating_add(row.saturating_mul(scale)),
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
        }
    }

    fn contents(&mut self, settings: &Settings) {
        if settings.display {
            let first = settings.selected.saturating_sub(2);
            for (row, (index, mode)) in settings.modes.iter().enumerate().skip(first).take(5).enumerate() {
                let y = 244usize.saturating_add(row.saturating_mul(64));
                if index == settings.selected {
                    self.rectangle(72, y.saturating_sub(12), 850, 54, [65, 43, 12, 255]);
                    self.rectangle(72, y.saturating_sub(12), 4, 54, CYAN);
                }
                self.text(
                    96,
                    y,
                    &format!("{} HZ", mode.replace('x', " X ").replace('@', " / ")),
                    4,
                    WHITE,
                );
                if index == settings.current {
                    self.text(720, y.saturating_add(6), "ACTIVE", 2, CYAN);
                }
            }
            if settings.modes.is_empty() {
                self.text(96, 256, "NO DRIVER MODES AVAILABLE", 3, AMBER);
            }
        } else {
            self.rectangle(72, 236, 1128, 76, [65, 43, 12, 255]);
            self.rectangle(72, 236, 4, 76, CYAN);
            self.text(96, 256, "DISPLAY", 5, WHITE);
            self.text(600, 268, "RESOLUTION / REFRESH", 3, CYAN);
        }
    }

    #[expect(clippy::float_arithmetic, reason = "countdown arc uses polar coordinates")]
    fn ring(&mut self, remaining: Duration) {
        let fraction = remaining.as_secs_f64() / 15.0;
        for y in 0_i32..180 {
            for x in 0_i32..180 {
                let delta_x = f64::from(x.saturating_sub(90));
                let delta_y = f64::from(y.saturating_sub(90));
                let radius = delta_x.hypot(delta_y);
                if (74.0..=82.0).contains(&radius) {
                    let angle = (delta_x.atan2(-delta_y) + core::f64::consts::TAU) % core::f64::consts::TAU;
                    let color = if angle / core::f64::consts::TAU <= fraction {
                        AMBER
                    } else {
                        [42, 51, 54, 255]
                    };
                    self.rectangle(
                        1000usize.saturating_add(usize::try_from(x).unwrap_or_default()),
                        460usize.saturating_add(usize::try_from(y).unwrap_or_default()),
                        1,
                        1,
                        color,
                    );
                }
            }
        }
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
pub fn buffer(settings: &Settings) -> Option<MemoryRenderBuffer> {
    if !settings.open {
        return None;
    }
    let mut canvas = Canvas(vec![0; WIDTH.saturating_mul(HEIGHT).saturating_mul(4)]);
    canvas.rectangle(0, 0, WIDTH, HEIGHT, [20, 14, 8, 235]);
    canvas.rectangle(48, 52, 6, 80, CYAN);
    canvas.text(80, 54, "TEHUTI / SYSTEM", 3, CYAN);
    canvas.text(
        80,
        92,
        if settings.display { "DISPLAY" } else { "SETTINGS" },
        6,
        WHITE,
    );
    canvas.rectangle(80, 166, 1120, 2, CYAN);
    canvas.text(80, 190, "POP THE HOOD", 2, AMBER);
    canvas.contents(settings);
    if let Some(remaining) = settings.remaining {
        canvas.ring(remaining);
        canvas.text(80, 604, "SOUTH: KEEP THIS MODE", 3, AMBER);
        canvas.text(80, 642, "AUTO REVERT IN 15 SECONDS", 2, WHITE);
    } else {
        if settings.failed() {
            canvas.text(80, 606, "MODE REJECTED / PREVIOUS MODE RESTORED", 2, AMBER);
        }
        canvas.text(80, 656, "D-PAD: MOVE   SOUTH: APPLY   EAST: BACK", 3, CYAN);
    }
    Some(MemoryRenderBuffer::from_slice(
        &canvas.0,
        Fourcc::Argb8888,
        (1280, 720),
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
            assert_eq!(element.src().size, (1280.0, 720.0).into());
        }
        Ok(())
    }
}
