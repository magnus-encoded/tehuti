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
    cached: Option<(Settings, (i32, i32), MemoryRenderBuffer)>,
}

impl Artwork {
    pub fn buffer(&mut self, settings: &Settings, size: (i32, i32)) -> Option<MemoryRenderBuffer> {
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
            self.cached = buffer(&key, size).map(|buffer| (key, size, buffer));
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

mod canvas;
use canvas::Canvas;

impl Canvas {
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
