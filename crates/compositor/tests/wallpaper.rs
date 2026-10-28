//! File wallpaper and shared output composition.

#[cfg(test)]
mod wallpaper {
    use core::error::Error;
    use smithay::{
        backend::{
            allocator::Fourcc,
            renderer::{
                Bind, ExportMem, Offscreen,
                damage::OutputDamageTracker,
                element::{Kind, memory::MemoryRenderBufferRenderElement},
                pixman::PixmanRenderer,
            },
        },
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        reexports::wayland_server::Display,
        utils::Transform,
    };
    use tehuti::{
        state::Compositor,
        wallpaper::{Wallpaper, WallpaperCache},
    };

    #[derive(Debug)]
    struct FailedArtwork(smithay::backend::renderer::element::Id);

    impl smithay::backend::renderer::element::Element for FailedArtwork {
        fn id(&self) -> &smithay::backend::renderer::element::Id {
            &self.0
        }
        fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
            smithay::backend::renderer::utils::CommitCounter::default()
        }
        fn src(&self) -> smithay::utils::Rectangle<f64, smithay::utils::Buffer> {
            smithay::utils::Rectangle::from_size((8.0, 4.0).into())
        }
        fn geometry(
            &self,
            _scale: smithay::utils::Scale<f64>,
        ) -> smithay::utils::Rectangle<i32, smithay::utils::Physical> {
            smithay::utils::Rectangle::from_size((8, 4).into())
        }
    }

    impl smithay::backend::renderer::element::RenderElement<PixmanRenderer> for FailedArtwork {
        fn draw(
            &self,
            frame: &mut smithay::backend::renderer::pixman::PixmanFrame<'_, '_>,
            _src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
            dst: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
            _damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
            _opaque_regions: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
        ) -> Result<(), smithay::backend::renderer::pixman::PixmanError> {
            use smithay::backend::renderer::Frame;
            frame.clear([1.0, 0.0, 0.0, 1.0].into(), &[dst])?;
            Err(smithay::backend::renderer::pixman::PixmanError::Unsupported)
        }
    }

    #[test]
    fn artwork_draw_failure_clears_partial_pixels_and_submits_next_frame() -> Result<(), Box<dyn Error>> {
        let mut renderer = PixmanRenderer::new()?;
        let mut image = renderer.create_buffer(Fourcc::Abgr8888, (8, 4).into())?;
        let mut target = renderer.bind(&mut image)?;
        let mut damage = OutputDamageTracker::new((8, 4), 1.0, Transform::Normal);
        let element = tehuti::wallpaper::WallpaperElement(FailedArtwork(
            smithay::backend::renderer::element::Id::new(),
        ));
        for _ in 0..2 {
            damage
                .render_output(&mut renderer, &mut target, 0, &[&element], [0.0, 0.0, 0.0, 1.0])?
                .sync
                .wait()?;
            let mapping = renderer.copy_framebuffer(
                &target,
                smithay::utils::Rectangle::from_size((8, 4).into()),
                Fourcc::Abgr8888,
            )?;
            assert!(
                renderer
                    .map_texture(&mapping)?
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| *pixel == [0, 0, 0, 255])
            );
        }
        Ok(())
    }

    #[test]
    fn missing_and_unreadable_files_draw_black() -> Result<(), Box<dyn Error>> {
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
                            .all(|pixel| *pixel == [0, 0, 0, 255])
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

    #[test]
    fn ready_wallpaper_requests_redraw_without_input() -> Result<(), Box<dyn Error>> {
        use smithay::reexports::calloop::{EventLoop, ping::make_ping};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("wallpaper.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([210, 80, 30, 255])).save(&path)?;
        let mut event_loop = EventLoop::<bool>::try_new()?;
        let (ping, source) = make_ping()?;
        let _token = event_loop
            .handle()
            .insert_source(source, |(), (), redraw| *redraw = true)?;
        let mut cache = WallpaperCache::new(path);
        cache.set_redraw_ping(ping);
        assert!(cache.buffer((20, 20).into()).is_none());
        let mut redraw = false;
        event_loop.dispatch(core::time::Duration::from_secs(2), &mut redraw)?;
        assert!(redraw, "ready wallpaper must request a redraw without input");
        assert!(cache.buffer((20, 20).into()).is_some());
        Ok(())
    }

    #[test]
    fn worker_prepares_artwork_and_reuses_it_after_file_removal() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("wallpaper.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([210, 80, 30, 255])).save(&path)?;
        let mut cache = WallpaperCache::new(path.clone());
        let deadline = std::time::Instant::now() + core::time::Duration::from_secs(10);
        let mut buffer = loop {
            if let Some(buffer) = cache.buffer((20, 20).into()) {
                break buffer;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "wallpaper worker must prepare artwork"
            );
            std::thread::sleep(core::time::Duration::from_millis(5));
        };
        std::fs::remove_file(path)?;
        buffer.render().draw(|pixels| {
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| *pixel == [210, 80, 30, 255])
            );
            Ok::<_, Box<dyn Error>>(Vec::new())
        })?;
        assert!(cache.buffer((20, 20).into()).is_some());
        Ok(())
    }

    fn capture(
        compositor: &Compositor,
        text: Option<&str>,
        cache: &std::sync::Mutex<WallpaperCache>,
    ) -> Result<image::RgbaImage, Box<dyn Error>> {
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
        tehuti::wallpaper::render_output_with_cache(
            &output,
            &mut renderer,
            &mut target,
            compositor,
            &menu,
            &mut damage,
            cache,
        )?
        .sync
        .wait()?;
        let mapping = renderer.copy_framebuffer(
            &target,
            smithay::utils::Rectangle::from_size((1280, 720).into()),
            Fourcc::Abgr8888,
        )?;
        let pixels = renderer.map_texture(&mapping)?.to_vec();
        image::RgbaImage::from_raw(1280, 720, pixels).ok_or_else(|| "invalid capture size".into())
    }

    #[test]
    fn prepared_wallpaper_draws_beneath_overlays() -> Result<(), Box<dyn Error>> {
        let display = Display::<Compositor>::new()?;
        let compositor = Compositor::new(&display.handle())?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("wallpaper.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([210, 80, 30, 255])).save(&path)?;
        let mut cache = WallpaperCache::new(path);
        let deadline = std::time::Instant::now() + core::time::Duration::from_secs(10);
        while cache.buffer((1280, 720).into()).is_none() {
            assert!(std::time::Instant::now() < deadline, "wallpaper must be ready");
            std::thread::sleep(core::time::Duration::from_millis(5));
        }
        let screenshot = capture(&compositor, Some("notice"), &std::sync::Mutex::new(cache))?;
        assert_eq!(*screenshot.get_pixel(16, 16), image::Rgba([32, 32, 32, 255]));
        assert_eq!(*screenshot.get_pixel(1000, 600), image::Rgba([210, 80, 30, 255]));
        Ok(())
    }

    #[test]
    fn unavailable_wallpaper_keeps_frames_and_overlays_rendering() -> Result<(), Box<dyn Error>> {
        let display = Display::<Compositor>::new()?;
        let compositor = Compositor::new(&display.handle())?;
        let directory = tempfile::tempdir()?;
        let cache = std::sync::Mutex::new(WallpaperCache::new(directory.path().join("missing.png")));
        for _ in 0..2 {
            let screenshot = capture(&compositor, Some("notice"), &cache)?;
            assert_eq!(*screenshot.get_pixel(16, 16), image::Rgba([32, 32, 32, 255]));
            assert_eq!(*screenshot.get_pixel(1000, 600), image::Rgba([0, 0, 0, 255]));
        }
        Ok(())
    }

    #[test]
    fn poisoned_wallpaper_cache_keeps_frames_and_overlays_rendering() -> Result<(), Box<dyn Error>> {
        let display = Display::<Compositor>::new()?;
        let compositor = Compositor::new(&display.handle())?;
        let directory = tempfile::tempdir()?;
        let cache = std::sync::Mutex::new(WallpaperCache::new(directory.path().join("missing.png")));
        std::thread::scope(|scope| {
            let _result = scope
                .spawn(|| {
                    let _guard = cache.lock();
                    std::panic::resume_unwind(Box::new("simulate wallpaper failure"));
                })
                .join();
        });
        for _ in 0..2 {
            let screenshot = capture(&compositor, Some("notice"), &cache)?;
            assert_eq!(*screenshot.get_pixel(16, 16), image::Rgba([32, 32, 32, 255]));
            assert_eq!(*screenshot.get_pixel(1000, 600), image::Rgba([0, 0, 0, 255]));
        }
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
    fn release_default_shows_locked_future_thumbnails() -> Result<(), Box<dyn Error>> {
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
        assert_eq!(first.names().len(), 5);
        assert_eq!(first.locks(), [false, true, true, true, true]);
        assert_eq!(first.thumbnails().len(), 5);
        assert_eq!(first.thumbnails()[4].dimensions(), (240, 135));
        assert_eq!(first.version(4), Some(4));
        assert!(first.select(4).is_err());
        let thumbnail = first.thumbnails()[4].as_ptr();
        first.unlock()?;
        assert_eq!(first.locks(), [false; 5]);
        assert_eq!(first.thumbnails()[4].as_ptr(), thumbnail);
        assert_eq!(red(&mut first)?, 10);
        std::fs::remove_file(directory.path().join("wallpapers-unlocked"))?;
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
        assert_eq!(red(&mut Collection::discover(&roots, store(), 4, false))?, 0);
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
