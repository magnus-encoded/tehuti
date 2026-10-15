//! File wallpaper and shared output composition.

#[cfg(test)]
mod wallpaper {
    use core::{error::Error, time::Duration};
    use smithay::{
        backend::{
            allocator::Fourcc,
            renderer::{
                Bind, Offscreen,
                damage::OutputDamageTracker,
                element::{Kind, memory::MemoryRenderBufferRenderElement},
                pixman::PixmanRenderer,
            },
        },
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        reexports::wayland_server::Display,
        utils::Transform,
    };
    use tehuti::{state::Compositor, wallpaper::Wallpaper};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    #[test]
    fn missing_and_unreadable_files_draw_near_black() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("wallpaper.png");
        for contents in [None, Some(b"invalid PNG".as_slice())] {
            if let Some(contents) = contents {
                std::fs::write(&path, contents)?;
            }
            let mut wallpaper = Wallpaper::load(&path);
            for size in [(8, 4), (4, 8), (8, 4)] {
                wallpaper.buffer(size.into())?.render().draw(|pixels| {
                    assert!(
                        pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .all(|pixel| *pixel == [12, 12, 12, 255])
                    );
                    Ok::<_, Box<dyn Error>>(Vec::new())
                })?;
            }
            assert!(wallpaper.buffer((0, 4).into()).is_err());
        }
        Ok(())
    }

    #[test]
    fn cover_crops_the_center_and_preserves_aspect_in_both_orientations() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("wallpaper.png");
        for transpose in [false, true] {
            let image = image::RgbaImage::from_fn(120, 40, |xpos, _| {
                image::Rgba(if (30..90).contains(&xpos) {
                    [210, 80, 30, 255]
                } else {
                    [0, 0, 255, 255]
                })
            });
            let image = if transpose {
                image::imageops::rotate90(&image)
            } else {
                image
            };
            image.save(&path)?;
            let mut wallpaper = Wallpaper::load(&path);
            std::fs::remove_file(&path)?;
            for size in [(20, 20), (80, 80), (20, 20)] {
                wallpaper.buffer(size.into())?.render().draw(|pixels| {
                    assert!(
                        pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .all(|pixel| *pixel == [210, 80, 30, 255])
                    );
                    assert_eq!(
                        pixels.len(),
                        usize::try_from(size.0.saturating_mul(size.1).saturating_mul(4))?
                    );
                    Ok::<_, Box<dyn Error>>(Vec::new())
                })?;
            }
        }
        Ok(())
    }

    fn capture(compositor: &Compositor, text: Option<&str>) -> Result<image::RgbaImage, Box<dyn Error>> {
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1280, 720).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        let mut renderer = PixmanRenderer::new()?;
        let mut image = renderer.create_buffer(Fourcc::Abgr8888, (1280, 720).into())?;
        let mut target = renderer.bind(&mut image)?;
        let menu_buffer = text.map(tehuti::notice::notice_buffer);
        let menu = menu_buffer
            .as_ref()
            .map(|buffer| {
                MemoryRenderBufferRenderElement::from_buffer(
                    &mut renderer,
                    (16.0, 16.0),
                    buffer,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )
            })
            .transpose()?
            .into_iter()
            .collect::<Vec<_>>();
        let mut damage = OutputDamageTracker::from_output(&output);
        tehuti::wallpaper::render_output(
            &output,
            &mut renderer,
            &mut target,
            compositor,
            &menu,
            &mut damage,
        )?
        .sync
        .wait()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("menu.png");
        tehuti::system::capture(&mut renderer, &target, (1280, 720), &path)?;
        Ok(image::open(path)?.to_rgba8())
    }

    #[test]
    fn menu_draws_above_settings_and_screenshot_includes_settings() -> Result<(), Box<dyn Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.handle(&Event::Speech {
            text: Text::new("Pop the hood.".into())?,
        });
        assert!(compositor.settings().open);
        compositor.handle(&Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Pressed,
        });
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_secs(3),
        });
        let text = compositor.visible_notice_text().ok_or("menu did not open")?;
        assert!(text.starts_with("SYSTEM MENU"));
        let screenshot = capture(&compositor, Some(&text))?;
        assert_eq!(*screenshot.get_pixel(16, 16), image::Rgba([32, 32, 32, 255]));
        assert_eq!(*screenshot.get_pixel(100, 166), image::Rgba([32, 32, 32, 255]));
        assert_eq!(*screenshot.get_pixel(800, 166), image::Rgba([64, 218, 240, 255]));
        let home = tempfile::tempdir()?;
        compositor.set_home(Some(home.path().to_path_buf()));
        for name in [ButtonName::DpadDown, ButtonName::South] {
            compositor.handle(&Event::Button {
                name,
                state: ButtonState::Pressed,
            });
        }
        assert!(compositor.take_screenshot_request().is_some());
        assert!(compositor.settings().open);
        let settings_screenshot = capture(&compositor, None)?;
        assert_eq!(
            *settings_screenshot.get_pixel(100, 166),
            image::Rgba([64, 218, 240, 255])
        );
        Ok(())
    }
}
