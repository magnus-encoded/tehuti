//! Couch-readable settings artwork.

use core::time::Duration;
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};
use tehuti_core::dispatch::{Category, Page, Settings};

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
    input: tehuti_core::dispatch::InputSettings,
    speech: String,
    hint: String,
    display_focus: tehuti_core::tiles::Focus,
    display_resolution: usize,
    current: usize,
    modes: Vec<String>,
    wallpapers: Vec<String>,
    wallpaper_locked: Vec<bool>,
    wallpaper_selected: usize,
    wallpaper_current: usize,
    bar_font: tehuti_core::bar::Font,
    scale: core::num::NonZeroU32,
    remaining: Option<Duration>,
    failed: bool,
    wallpaper_revision: u64,
}

impl View {
    fn from_settings(settings: &Settings, speech: &str) -> Self {
        Self {
            page: settings.page,
            category: settings.category,
            input: settings.input.clone(),
            speech: speech.to_owned(),
            hint: settings.button_hint(),
            display_focus: settings.display_focus,
            display_resolution: settings.display_resolution,
            current: settings.current,
            modes: settings.modes.clone(),
            wallpapers: settings.wallpapers.clone(),
            wallpaper_locked: settings.wallpaper_locked.clone(),
            wallpaper_selected: settings.wallpaper_selected,
            wallpaper_current: settings.wallpaper_current,
            bar_font: settings.bar_font,
            scale: settings.scale,
            remaining: settings.remaining.map(|remaining| {
                Duration::from_millis(
                    u64::try_from(remaining.as_millis() / 100)
                        .unwrap_or_default()
                        .saturating_mul(100),
                )
            }),
            failed: settings.failed(),
            wallpaper_revision: crate::wallpaper::revision(),
        }
    }
}

impl Artwork {
    pub fn buffer(
        &mut self,
        settings: &Settings,
        speech: &str,
        size: (i32, i32),
    ) -> Option<MemoryRenderBuffer> {
        if !settings.open {
            self.cached = None;
            return None;
        }
        let view = View::from_settings(settings, speech);
        if self
            .cached
            .as_ref()
            .is_none_or(|(previous, previous_size, _)| *previous != view || *previous_size != size)
        {
            let mut drawn = settings.clone();
            drawn.remaining = view.remaining;
            self.cached = buffer(&drawn, speech, size).map(|buffer| (view, size, buffer));
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
mod pages;
mod tiles;
use canvas::Canvas;

impl Canvas {
    fn contents(&mut self, settings: &Settings, speech: &str) {
        match settings.page {
            Page::Categories => self.categories(settings.category),
            Page::Display => self.display(settings),
            Page::Input => self.input(settings, speech),
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
                Category::Display => ("DISPLAY", "RESOLUTION / REFRESH / SCALE / BAR FONT"),
                Category::Input => ("INPUT", "CONTROLLER / MICROPHONE"),
                Category::Wallpaper => ("WALLPAPER", "RELEASE COLLECTION"),
            };
            self.text(96, y.saturating_add(20), name, 5, WHITE);
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

/// The running version and short commit SHA, as `v0.134.1 · 1a2b3c4`.
#[must_use]
fn version_line(version: &str, commit: &str) -> String {
    format!("v{version} \u{b7} {commit}")
}

#[must_use]
fn build_line() -> String {
    version_line(
        option_env!("TEHUTI_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        env!("TEHUTI_GIT_HASH"),
    )
}

#[must_use]
pub fn buffer(settings: &Settings, speech: &str, size: (i32, i32)) -> Option<MemoryRenderBuffer> {
    if !settings.open {
        return None;
    }
    let mut canvas = Canvas::new(size)?;
    canvas.rectangle(0, 0, WIDTH, HEIGHT, [20, 14, 8, 255]);
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
    if settings.page != Page::Wallpaper {
        canvas.text(80, 190, "POP THE HOOD", 2, AMBER);
    }
    if settings.page == Page::Categories {
        canvas.text(1200 - 24 * 15, 186, &build_line(), 3, WHITE);
    }
    canvas.contents(settings, speech);
    if let Some(remaining) = settings.remaining {
        canvas.ring(remaining);
        canvas.text(80, 604, &settings.button_hint(), 3, AMBER);
        canvas.text(80, 642, "AUTO REVERT IN 15 SECONDS", 2, WHITE);
    } else {
        if settings.failed() && settings.page == Page::Display {
            canvas.text(80, 606, "MODE REJECTED / PREVIOUS MODE RESTORED", 2, AMBER);
        }
        canvas.text(80, 656, &settings.button_hint(), 3, CYAN);
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
    fn the_top_page_shows_the_compiled_in_version_and_commit() {
        assert_eq!(
            super::version_line("0.134.1", "1a2b3c4"),
            "v0.134.1 \u{b7} 1a2b3c4"
        );
        let line = super::build_line();
        let version = option_env!("TEHUTI_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        assert_eq!(line, format!("v{version} \u{b7} {}", env!("TEHUTI_GIT_HASH")));
        assert_ne!(env!("TEHUTI_GIT_HASH"), "");
    }

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
            tehuti_protocol::ButtonName::DpadRight,
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

    fn input_test_controllers() -> Vec<tehuti_core::dispatch::ControllerInfo> {
        crate::controller::numbered_controllers(
            [
                (
                    "/dev/input/event2".into(),
                    "Wireless Controller".into(),
                    0x054c,
                    0x09cc,
                    String::new(),
                    tehuti_core::dispatch::Connection::Usb,
                ),
                (
                    "/dev/input/event9".into(),
                    "Xbox Controller".into(),
                    0x045e,
                    0x0b13,
                    String::new(),
                    tehuti_core::dispatch::Connection::Usb,
                ),
            ],
            &std::collections::BTreeMap::new(),
        )
    }

    #[test]
    fn the_input_page_disables_a_pad_and_saves_it_with_its_binding_and_microphone()
    -> Result<(), Box<dyn core::error::Error>> {
        use tehuti_core::dispatch::MicrophoneChoice;
        use tehuti_protocol::{ButtonName, ButtonState};
        let directory = tempfile::tempdir()?;
        let store = crate::input_choice_store::Store::new(directory.path().join("tehuti"));
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.load_input_choices(store.clone());
        compositor.configure_controllers(input_test_controllers());
        compositor.activate_controller("Wireless Controller#1".into());
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
        assert!(
            compositor.controller_chosen("Xbox Controller#1"),
            "moving disables nothing"
        );
        press(&mut compositor, ButtonName::South);
        assert!(!compositor.controller_chosen("Xbox Controller#1"));
        assert!(compositor.controller_chosen("Wireless Controller#1"));
        assert_eq!(
            store.disabled_controllers()?,
            vec!["Xbox Controller#1".to_owned()]
        );
        press(&mut compositor, ButtonName::South);
        assert!(compositor.controller_chosen("Xbox Controller#1"));
        assert_eq!(store.disabled_controllers()?, Vec::<String>::new());
        press(&mut compositor, ButtonName::DpadDown);
        for _ in 0..3 {
            press(&mut compositor, ButtonName::DpadRight);
        }
        press(&mut compositor, ButtonName::South);
        assert_eq!(
            store.controller_bindings()?.get("Xbox Controller#1"),
            Some(&ButtonName::Start)
        );
        press(&mut compositor, ButtonName::DpadLeft);
        press(&mut compositor, ButtonName::DpadLeft);
        press(&mut compositor, ButtonName::DpadDown);
        press(&mut compositor, ButtonName::South);
        assert_eq!(store.microphone()?, MicrophoneChoice::Auto);
        Ok(())
    }

    #[test]
    fn a_saved_single_pad_choice_never_rejects_other_enabled_pads() -> Result<(), Box<dyn core::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let store = crate::input_choice_store::Store::new(directory.path().join("tehuti"));
        store.set_controller(&tehuti_core::dispatch::ControllerChoice::Named(
            "Wireless Controller".into(),
        ))?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.load_input_choices(store);
        compositor.configure_controllers(input_test_controllers());
        assert!(compositor.controller_chosen("Wireless Controller#1"));
        assert!(compositor.controller_chosen("Xbox Controller#1"));
        Ok(())
    }

    #[test]
    fn a_saved_disabled_pad_stays_disabled_after_a_restart() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = crate::input_choice_store::Store::new(directory.path().join("tehuti"));
        store.set_disabled_controllers(&["Xbox Controller".to_owned()])?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.load_input_choices(store);
        assert!(!compositor.controller_chosen("Xbox Controller"));
        assert!(compositor.controller_chosen("Wireless Controller"));
        assert!(!compositor.settings().input.controller_enabled("Xbox Controller"));
        Ok(())
    }
}
