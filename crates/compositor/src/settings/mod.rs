//! Couch-readable settings artwork.

use core::time::Duration;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use tehuti_core::dispatch::{Category, InputRow, Page, Settings};

#[derive(Debug, Default)]
pub(crate) struct Display {
    pub requests: std::collections::VecDeque<(usize, bool)>,
    pub artwork: core::cell::RefCell<Artwork>,
}

#[derive(Debug, Default)]
pub struct Artwork {
    cached: Option<(View, (i32, i32), MemoryRenderBuffer)>,
}

/// What the artwork draws. Rasterising a frame takes about 100 ms on a slow CPU,
/// so input bookkeeping such as held buttons never forces a redraw.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    page: Page,
    category: Category,
    row: InputRow,
    controller: String,
    microphone: String,
    hint: &'static str,
    selected: usize,
    current: usize,
    modes: Vec<String>,
    wallpapers: Vec<String>,
    wallpaper_locked: Vec<bool>,
    wallpaper_selected: usize,
    wallpaper_current: usize,
    scale: core::num::NonZeroU32,
    remaining: Option<Duration>,
    failed: bool,
}

impl View {
    fn from_settings(settings: &Settings) -> Self {
        Self {
            page: settings.page,
            category: settings.category,
            row: settings.input.row,
            controller: settings.input.controller_label(),
            microphone: settings.input.microphone_label(),
            hint: settings.button_hint(),
            selected: settings.selected,
            current: settings.current,
            modes: settings.modes.clone(),
            wallpapers: settings.wallpapers.clone(),
            wallpaper_locked: settings.wallpaper_locked.clone(),
            wallpaper_selected: settings.wallpaper_selected,
            wallpaper_current: settings.wallpaper_current,
            scale: settings.scale,
            remaining: settings.remaining.map(|remaining| {
                Duration::from_millis(
                    u64::try_from(remaining.as_millis() / 100)
                        .unwrap_or_default()
                        .saturating_mul(100),
                )
            }),
            failed: settings.failed(),
        }
    }
}

impl Artwork {
    pub fn buffer(&mut self, settings: &Settings, size: (i32, i32)) -> Option<MemoryRenderBuffer> {
        if !settings.open {
            self.cached = None;
            return None;
        }
        let view = View::from_settings(settings);
        if self
            .cached
            .as_ref()
            .is_none_or(|(previous, previous_size, _)| *previous != view || *previous_size != size)
        {
            let mut drawn = settings.clone();
            drawn.remaining = view.remaining;
            self.cached = buffer(&drawn, size).map(|buffer| (view, size, buffer));
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
    fn contents(&mut self, settings: &Settings) {
        match settings.page {
            Page::Categories => self.categories(settings.category),
            Page::Display => self.display(settings),
            Page::Input => self.input(settings),
            Page::Wallpaper => self.wallpaper(settings),
        }
    }

    fn highlight(&mut self, y: usize, height: usize) {
        self.rectangle(72, y, 1128, height, [65, 43, 12, 255]);
        self.rectangle(72, y, 4, height, CYAN);
    }

    fn categories(&mut self, selected: Category) {
        for (row, category) in Category::ALL.into_iter().enumerate() {
            let y = 236usize.saturating_add(row.saturating_mul(96));
            if category == selected {
                self.highlight(y, 76);
            }
            let (name, detail) = match category {
                Category::Display => ("DISPLAY", "RESOLUTION / REFRESH / SCALE"),
                Category::Input => ("INPUT", "CONTROLLER / MICROPHONE"),
                Category::Wallpaper => ("WALLPAPER", "RELEASE COLLECTION"),
            };
            self.text(96, y.saturating_add(20), name, 5, WHITE);
            self.text(600, y.saturating_add(32), detail, 3, CYAN);
        }
    }

    fn display(&mut self, settings: &Settings) {
        let first = settings.selected.saturating_sub(2);
        let rows: Vec<String> = settings
            .modes
            .iter()
            .map(|mode| format!("{} HZ", mode.replace('x', " X ").replace('@', " / ")))
            .chain(core::iter::once(format!(
                "SCALE {}  (LEFT / RIGHT)",
                settings.scale
            )))
            .collect();
        for (row, (index, mode)) in rows.iter().enumerate().skip(first).take(5).enumerate() {
            let y = 244usize.saturating_add(row.saturating_mul(64));
            if index == settings.selected {
                self.rectangle(72, y.saturating_sub(12), 850, 54, [65, 43, 12, 255]);
                self.rectangle(72, y.saturating_sub(12), 4, 54, CYAN);
            }
            self.text(96, y, mode, 4, WHITE);
            if index == settings.current && index < settings.modes.len() {
                self.text(720, y.saturating_add(6), "ACTIVE", 2, CYAN);
            }
        }
        if settings.modes.is_empty() {
            self.text(96, 568, "NO DRIVER MODES AVAILABLE", 3, AMBER);
        }
        if let Some(mode) = settings.modes.get(settings.selected) {
            self.mode_preview(mode);
        }
    }

    fn mode_preview(&mut self, mode: &str) {
        let Some((size, refresh)) = mode.split_once('@') else {
            return;
        };
        let Some((width, height)) = size.split_once('x') else {
            return;
        };
        let (Ok(width), Ok(height), Ok(refresh)) = (
            width.parse::<usize>(),
            height.parse::<usize>(),
            refresh.parse::<f32>(),
        ) else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        let preview_width = 200.min(width.saturating_mul(120).checked_div(height).unwrap_or_default());
        let preview_height = 120.min(height.saturating_mul(200).checked_div(width).unwrap_or_default());
        self.rectangle(960, 230, preview_width, 2, CYAN);
        self.rectangle(960, 230, 2, preview_height, CYAN);
        self.rectangle(
            960,
            230usize.saturating_add(preview_height),
            preview_width,
            2,
            CYAN,
        );
        self.rectangle(
            960usize.saturating_add(preview_width),
            230,
            2,
            preview_height,
            CYAN,
        );
        self.wave(960, 445, refresh);
    }

    fn input(&mut self, settings: &Settings) {
        let rows = [
            (
                InputRow::Controller,
                "CONTROLLER",
                settings.input.controller_label(),
            ),
            (
                InputRow::Microphone,
                "MICROPHONE",
                settings.input.microphone_label(),
            ),
        ];
        for (index, (row, name, value)) in rows.into_iter().enumerate() {
            let y = 230usize.saturating_add(index.saturating_mul(124));
            if row == settings.input.row {
                self.highlight(y, 108);
            }
            self.text(96, y.saturating_add(14), name, 4, WHITE);
            let value: String = value.chars().take(36).collect();
            self.text(96, y.saturating_add(66), &format!("<  {value}  >"), 3, CYAN);
        }
    }

    fn wallpaper(&mut self, settings: &Settings) {
        if settings.wallpapers.is_empty() {
            self.text(96, 300, "NO WALLPAPERS AVAILABLE", 4, AMBER);
            return;
        }
        let first = settings.wallpaper_selected.saturating_sub(2);
        for (row, (index, name)) in settings
            .wallpapers
            .iter()
            .enumerate()
            .skip(first)
            .take(5)
            .enumerate()
        {
            let y = 220usize.saturating_add(row.saturating_mul(72));
            if index == settings.wallpaper_selected {
                self.highlight(y, 64);
            }
            let locked = settings.wallpaper_locked.get(index).copied().unwrap_or(true);
            self.text(
                96,
                y.saturating_add(12),
                &name.to_uppercase(),
                4,
                if locked { AMBER } else { WHITE },
            );
            if locked {
                self.text(560, y.saturating_add(18), "LOCKED", 2, AMBER);
            }
            if index == settings.wallpaper_current {
                self.text(560, y.saturating_add(18), "SAVED", 2, CYAN);
            }
        }
        if let Some(image) = crate::wallpaper::thumbnail(settings.wallpaper_selected) {
            self.thumbnail(
                760,
                230,
                &image,
                settings
                    .wallpaper_locked
                    .get(settings.wallpaper_selected)
                    .copied()
                    .unwrap_or(true),
            );
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
        match settings.page {
            Page::Categories => "SETTINGS",
            Page::Display => "DISPLAY",
            Page::Input => "INPUT",
            Page::Wallpaper => "WALLPAPER",
        },
        6,
        WHITE,
    );
    canvas.rectangle(80, 166, 1120, 2, CYAN);
    canvas.text(80, 190, "POP THE HOOD", 2, AMBER);
    canvas.contents(settings);
    if let Some(remaining) = settings.remaining {
        canvas.ring(remaining);
        canvas.text(80, 604, settings.button_hint(), 3, AMBER);
        canvas.text(80, 642, "AUTO REVERT IN 15 SECONDS", 2, WHITE);
    } else {
        if settings.failed() && settings.page == Page::Display {
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
                state: tehuti_protocol::ButtonState::Released,
            });
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

    #[test]
    fn artwork_is_rasterised_again_only_when_what_it_shows_changes() -> Result<(), Box<dyn core::error::Error>>
    {
        use tehuti_protocol::{ButtonName, ButtonState};
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
        output.change_current_state(
            Some(Mode {
                size: (1024, 600).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        let mut renderer = PixmanRenderer::new()?;
        let drawn = |target: &mut PixmanRenderer,
                     state: &Compositor|
         -> Result<smithay::backend::renderer::element::Id, Box<dyn core::error::Error>> {
            Ok(super::element(target, state, &output)?
                .ok_or("settings element unavailable")?
                .id()
                .clone())
        };
        let opened = drawn(&mut renderer, &compositor)?;
        for state in [ButtonState::Pressed, ButtonState::Released] {
            compositor.handle(&Event::Button {
                name: ButtonName::North,
                state,
            });
        }
        assert_eq!(
            drawn(&mut renderer, &compositor)?,
            opened,
            "an ignored button leaves the artwork alone"
        );
        compositor.handle(&Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        });
        assert_eq!(compositor.settings().page, tehuti_core::dispatch::Page::Display);
        assert_ne!(
            drawn(&mut renderer, &compositor)?,
            opened,
            "entering the display page redraws"
        );
        Ok(())
    }

    #[test]
    fn the_input_page_previews_then_saves_one_pad_and_a_microphone() -> Result<(), Box<dyn core::error::Error>>
    {
        use tehuti_core::dispatch::{ControllerChoice, MicrophoneChoice};
        use tehuti_protocol::{ButtonName, ButtonState};
        let directory = tempfile::tempdir()?;
        let store = crate::input_choice_store::Store::new(directory.path().join("tehuti"));
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.load_input_choices(store.clone());
        compositor.configure_controllers(vec!["Wireless Controller".into(), "Xbox Controller".into()]);
        compositor.handle(&Event::Speech {
            text: Text::new("Pop the hood.".into())?,
        });
        let press = |target: &mut Compositor, name| {
            for state in [ButtonState::Released, ButtonState::Pressed] {
                target.handle(&Event::Button { name, state });
            }
        };
        press(&mut compositor, ButtonName::DpadDown);
        press(&mut compositor, ButtonName::South);
        assert_eq!(compositor.settings().page, tehuti_core::dispatch::Page::Input);
        assert!(compositor.settings_buffer((1024, 600)).is_some());
        press(&mut compositor, ButtonName::DpadRight);
        assert!(compositor.controller_chosen("Wireless Controller"));
        assert!(!compositor.controller_chosen("Xbox Controller"));
        assert_eq!(
            store.controller()?,
            ControllerChoice::Auto,
            "a preview is not saved"
        );
        press(&mut compositor, ButtonName::South);
        assert_eq!(
            store.controller()?,
            ControllerChoice::Named("Wireless Controller".into())
        );
        press(&mut compositor, ButtonName::DpadDown);
        press(&mut compositor, ButtonName::DpadRight);
        assert_eq!(store.microphone()?, MicrophoneChoice::Auto);
        compositor.configure_controllers(vec!["Xbox Controller".into()]);
        assert!(
            compositor.controller_chosen("Xbox Controller"),
            "every pad drives while the chosen one is away"
        );
        Ok(())
    }
}
