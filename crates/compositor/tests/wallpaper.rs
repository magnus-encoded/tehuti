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
