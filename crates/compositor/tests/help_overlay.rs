//! Compositor integration, artwork and screenshot tests for the "what can I say / press" overlay.

#[cfg(test)]
mod help_overlay {
    use core::time::Duration;
    use smithay::{
        backend::renderer::{element::Element, pixman::PixmanRenderer},
        output::{Mode, Output, PhysicalProperties, Subpixel},
        reexports::wayland_server::Display,
    };
    use tehuti::{
        input::{SyntheticPad, route},
        state::Compositor,
    };
    use tehuti_core::{help_overlay::GroupKind, pad::PadFamily};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text, parse_context};

    const GLOBAL: &str = r#"
app_id = "*"
push_to_talk = ["record"]

[buttons]
east = { key = "escape" }
left_bumper = { layout = "focus-column-left" }
right_bumper = { layout = "focus-column-right" }

[speech]
"Close window." = { close = "focused" }
"Volume down." = { launch = "tehuti-volume-down.desktop" }
"#;

    const EDITOR: &str = r#"
app_id = "org.example.Editor"
label = "Editor"

[buttons]
south = { key = "s", mods = ["ctrl"] }
west = { key = "o", mods = ["ctrl"] }
north = { key = "f1" }

[sticks.left]
up = { key = "up" }
down = { key = "down" }

[speech]
"Save file." = { key = "s", mods = ["ctrl"], icon = "💾" }
"Save." = { key = "s", mods = ["ctrl"], icon = "💾" }
"Open file." = { key = "o", mods = ["ctrl"], icon = "📂" }
"Pause." = { key = "p", mods = ["ctrl"], icon = "⏸" }
"#;

    fn output(size: (i32, i32)) -> Output {
        let output = Output::new(
            "example".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Example".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: size.into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        output
    }

    fn tap(compositor: &mut Compositor, pad: &mut SyntheticPad, name: ButtonName) -> std::io::Result<()> {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pad.push(Event::Button { name, state });
        }
        route(compositor, pad)
    }

    fn compositor_with_editor() -> Result<(Display<Compositor>, Compositor), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        let contexts = [GLOBAL, EDITOR]
            .into_iter()
            .map(parse_context)
            .collect::<Result<Vec<_>, _>>()?;
        compositor.set_contexts(contexts);
        compositor.handle(&Event::Focus {
            app_id: Some("org.example.Editor".into()),
            title: String::new(),
        });
        Ok((display, compositor))
    }

    #[test]
    fn voice_and_synthetic_pad_drive_the_cached_overlay_element() -> Result<(), Box<dyn core::error::Error>> {
        let (_display, mut compositor) = compositor_with_editor()?;
        let out = output((1280, 720));
        let mut renderer = PixmanRenderer::new()?;
        let mut buffer = tehuti::help_overlay::Buffer::default();
        assert!(
            buffer.element(&mut renderer, &compositor, &out)?.is_none(),
            "closed overlay produces no render element"
        );

        compositor.handle(&Event::Speech {
            text: Text::new("What can I say?".into())?,
        });
        assert!(compositor.help_overlay().is_some());
        let directory = tempfile::tempdir()?;
        let mut driver = tehuti::driver::Driver::bind(directory.path(), tehuti::backend::Backend::Nested)?;
        let scene = driver.execute(&mut compositor, tehuti::driver::Request::Scene)?;
        assert_eq!(scene["overlays"]["help_overlay"], serde_json::json!(true));

        let first = buffer
            .element(&mut renderer, &compositor, &out)?
            .ok_or("missing overlay")?
            .id()
            .clone();
        let cached = buffer
            .element(&mut renderer, &compositor, &out)?
            .ok_or("missing cached overlay")?
            .id()
            .clone();
        assert_eq!(first, cached, "unchanged overlay keeps its element identity");

        let mut pad = SyntheticPad::default();
        tap(&mut compositor, &mut pad, ButtonName::DpadRight)?;
        assert_eq!(
            compositor
                .help_overlay()
                .and_then(tehuti_core::help_overlay::HelpOverlay::active_group)
                .map(|group| group.kind),
            Some(GroupKind::FocusedPad)
        );
        let moved = buffer
            .element(&mut renderer, &compositor, &out)?
            .ok_or("missing updated overlay")?
            .id()
            .clone();
        assert_ne!(first, moved, "switching groups redraws the overlay");

        tap(&mut compositor, &mut pad, ButtonName::East)?;
        assert!(compositor.help_overlay().is_none());
        assert!(buffer.element(&mut renderer, &compositor, &out)?.is_none());
        Ok(())
    }

    #[test]
    fn guide_menu_opens_overlay_and_confirm_runs_built_in_unit() -> Result<(), Box<dyn core::error::Error>> {
        let (_display, mut compositor) = compositor_with_editor()?;
        let mut pad = SyntheticPad::default();
        pad.push(Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Pressed,
        });
        route(&mut compositor, &mut pad)?;
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(500),
        });
        pad.push(Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Released,
        });
        route(&mut compositor, &mut pad)?;

        let menu_text = compositor.visible_notice_text().ok_or("system menu missing")?;
        let row = menu_text
            .lines()
            .skip(1)
            .position(|line| line.trim_start_matches(['>', ' ']) == "WHAT CAN I SAY")
            .ok_or("WHAT CAN I SAY missing from system menu")?;
        for _ in 0..row {
            tap(&mut compositor, &mut pad, ButtonName::DpadDown)?;
        }
        tap(&mut compositor, &mut pad, ButtonName::South)?;
        assert!(
            compositor.help_overlay().is_some(),
            "Guide menu entry opens help overlay"
        );
        assert!(compositor.visible_notice_text().is_none());

        // Step to GlobalVoice and confirm "keyboard".
        tap(&mut compositor, &mut pad, ButtonName::DpadRight)?;
        tap(&mut compositor, &mut pad, ButtonName::DpadRight)?;
        let keyboard_row = compositor
            .help_overlay()
            .and_then(tehuti_core::help_overlay::HelpOverlay::active_group)
            .and_then(|group| {
                group
                    .items
                    .iter()
                    .position(|item| item.voice_name.as_deref() == Some("keyboard"))
            })
            .ok_or("keyboard entry missing")?;
        for _ in 0..keyboard_row {
            tap(&mut compositor, &mut pad, ButtonName::DpadDown)?;
        }
        tap(&mut compositor, &mut pad, ButtonName::South)?;
        assert!(compositor.help_overlay().is_none());
        assert!(
            compositor.on_screen_keyboard().is_open(),
            "confirming keyboard opens the on-screen keyboard"
        );
        Ok(())
    }

    #[test]
    fn panel_centres_above_the_bar_across_sizes_and_scales() -> Result<(), Box<dyn core::error::Error>> {
        let (_display, mut compositor) = compositor_with_editor()?;
        compositor.handle(&Event::Speech {
            text: Text::new("What can I say".into())?,
        });
        let overlay = compositor.help_overlay().ok_or("missing overlay")?;
        for (size, scale) in [
            ((1280, 720), 1),
            ((1920, 1080), 1),
            ((1920, 1080), 2),
            ((2560, 1440), 1),
            ((640, 360), 1),
        ] {
            let pixels = tehuti::help_overlay::raster_scaled(overlay, PadFamily::Letters, size, scale)
                .ok_or("artwork")?;
            assert_eq!(pixels.get(0..4), Some([0, 0, 0, 0].as_slice()));
            let width = usize::try_from(size.0)?;
            let height = usize::try_from(size.1)?;
            let bar = usize::try_from(tehuti_core::bar::HEIGHT.saturating_mul(scale))?;
            let opaque: Vec<_> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, pixel)| pixel[3] >= 200)
                .map(|(index, _)| {
                    (
                        index.checked_rem(width).unwrap_or_default(),
                        index.checked_div(width).unwrap_or_default(),
                    )
                })
                .collect();
            let left = opaque.iter().map(|point| point.0).min().ok_or("left")?;
            let right = opaque.iter().map(|point| point.0).max().ok_or("right")?;
            let bottom = opaque.iter().map(|point| point.1).max().ok_or("bottom")?;
            assert!(
                left.abs_diff(width.saturating_sub(right).saturating_sub(1)) <= 1,
                "horizontally centred at {size:?} scale {scale}"
            );
            assert!(
                bottom < height.saturating_sub(bar),
                "stops above the bar at {size:?} scale {scale}"
            );
            let colours: std::collections::HashSet<_> = pixels.as_chunks::<4>().0.iter().copied().collect();
            assert!(colours.len() > 40, "smooth anti-aliased text coverage");
        }
        Ok(())
    }

    /// Writes screenshots to `$SHOT_DIR` when set so reviewers can inspect the rendered overlay.
    #[test]
    fn help_overlay_screenshots_can_be_written() -> Result<(), Box<dyn core::error::Error>> {
        let Some(directory) = std::env::var_os("SHOT_DIR") else {
            return Ok(());
        };
        let (_display, mut compositor) = compositor_with_editor()?;
        compositor.handle(&Event::Speech {
            text: Text::new("What can I say".into())?,
        });
        let mut pad = SyntheticPad::default();
        for (name, step_right) in [
            ("help-overlay-voice.png", false),
            ("help-overlay-pad.png", true),
            ("help-overlay-global-voice.png", true),
        ] {
            if step_right {
                tap(&mut compositor, &mut pad, ButtonName::DpadRight)?;
            }
            let overlay = compositor.help_overlay().ok_or("missing overlay")?;
            let bgra = tehuti::help_overlay::raster(overlay, PadFamily::Letters, (1280, 720))
                .ok_or("missing raster")?;
            let mut rgba = Vec::with_capacity(bgra.len());
            for pixel in bgra.as_chunks::<4>().0 {
                // Composite over a dark desktop background so the PNG is easy to inspect.
                let alpha = u16::from(pixel[3]);
                let blend = |foreground: u8, background: u8| {
                    let mixed = u16::from(foreground)
                        .saturating_mul(alpha)
                        .saturating_add(u16::from(background).saturating_mul(255u16.saturating_sub(alpha)))
                        .checked_div(255)
                        .unwrap_or_default();
                    u8::try_from(mixed).unwrap_or_default()
                };
                rgba.extend_from_slice(&[blend(pixel[2], 18), blend(pixel[1], 22), blend(pixel[0], 28), 255]);
            }
            let path = std::path::Path::new(&directory).join(name);
            image::save_buffer(path, &rgba, 1280, 720, image::ColorType::Rgba8)?;
        }
        Ok(())
    }
}
