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

#[cfg(test)]
mod collection {
    use core::error::Error;
    use std::path::Path;
    use tehuti::{display_mode_store::Store, wallpaper::Collection};

    fn image(root: &Path, name: &str, red: u8) -> Result<(), Box<dyn Error>> {
        std::fs::create_dir_all(root.join("wallpapers"))?;
        image::RgbaImage::from_pixel(2, 2, image::Rgba([red, 0, 0, 255]))
            .save(root.join("wallpapers").join(format!("{name}.png")))?;
        Ok(())
    }

    fn red(collection: &mut Collection) -> Result<u8, Box<dyn Error>> {
        let mut result = 0;
        collection.buffer((2, 2).into())?.render().draw(|pixels| {
            result = pixels.first().copied().ok_or("empty buffer")?;
            Ok::<_, Box<dyn Error>>(Vec::new())
        })?;
        Ok(result)
    }

    #[test]
    fn release_default_and_gradual_unlock_ignore_future_images() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        for (name, color) in [
            ("apteribis", 10),
            ("bostrychia", 20),
            ("cercibis", 30),
            ("eudocimus", 40),
            ("geronticus", 50),
        ] {
            image(directory.path(), name, color)?;
        }
        let roots = [directory.path().to_path_buf()];
        let store = || Store::new(directory.path().join("selection"));
        let mut first = Collection::discover(&roots, store(), 0, false);
        assert_eq!(first.names(), ["apteribis"]);
        assert_eq!(red(&mut first)?, 10);
        let mut fourth = Collection::discover(&roots, store(), 4, false);
        assert_eq!(
            fourth.names(),
            ["apteribis", "bostrychia", "cercibis", "eudocimus", "geronticus"]
        );
        assert_eq!(red(&mut fourth)?, 50);
        assert_eq!(Collection::discover(&roots, store(), 0, true).names().len(), 5);
        store().write("geronticus")?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 0, false))?, 10);
        Ok(())
    }

    #[test]
    fn lookup_prefers_user_then_installed_then_first_available_then_plain() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let user = directory.path().join("user");
        let installed = directory.path().join("installed");
        let roots = [user.clone(), installed.clone()];
        let store = || Store::new(directory.path().join("selection"));
        image(&installed, "geronticus", 50)?;
        image(&user, "geronticus", 60)?;
        image(&installed, "apteribis", 10)?;
        image(&user, "bostrychia", 20)?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 4, false))?, 60);
        std::fs::write(user.join("wallpapers/geronticus.png"), "invalid")?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 4, false))?, 50);
        std::fs::remove_file(installed.join("wallpapers/geronticus.png"))?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 4, false))?, 10);
        std::fs::remove_file(installed.join("wallpapers/apteribis.png"))?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 4, false))?, 20);
        std::fs::remove_file(user.join("wallpapers/bostrychia.png"))?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 4, false))?, 12);
        image::RgbaImage::from_pixel(2, 2, image::Rgba([80, 0, 0, 255])).save(user.join("wallpaper.png"))?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 4, false))?, 80);
        Ok(())
    }

    #[test]
    fn applied_selection_replaces_cached_pixels_and_survives_startup() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        image(directory.path(), "apteribis", 10)?;
        image(directory.path(), "bostrychia", 20)?;
        let roots = [directory.path().to_path_buf()];
        let store = || Store::new(directory.path().join("config/wallpaper"));
        image::RgbaImage::from_pixel(2, 2, image::Rgba([80, 0, 0, 255]))
            .save(directory.path().join("wallpaper.png"))?;
        let mut collection = Collection::discover(&roots, store(), 0, true);
        assert_eq!(red(&mut collection)?, 80);
        collection.select(1)?;
        assert_eq!(red(&mut collection)?, 20);
        assert_eq!(store().read()?.as_deref(), Some("bostrychia"));
        assert_eq!(red(&mut Collection::discover(&roots, store(), 0, true))?, 20);
        std::fs::remove_file(directory.path().join("wallpaper.png"))?;
        store().write("../../untrusted")?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 0, true))?, 10);
        Ok(())
    }
}
