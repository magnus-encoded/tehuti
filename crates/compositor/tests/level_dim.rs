//! Level dimming through real Wayland surfaces and the production output renderer.

#[cfg(test)]
mod level_dim {
    use core::time::Duration;
    use std::{
        io::Write,
        os::{fd::AsFd, unix::net::UnixStream},
        sync::Arc,
        sync::mpsc,
        thread,
        time::Instant,
    };

    use smithay::{
        output::{Mode, Output, PhysicalProperties, Subpixel},
        reexports::wayland_server::Display,
    };
    use tehuti::{
        scale,
        state::{ClientState, Compositor},
    };
    use wayland_client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_buffer::WlBuffer,
            wl_compositor::WlCompositor,
            wl_output::WlOutput,
            wl_registry::WlRegistry,
            wl_shm::{self, WlShm},
            wl_shm_pool::WlShmPool,
            wl_surface::WlSurface,
        },
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::{self, XdgToplevel},
        xdg_wm_base::{self, XdgWmBase},
    };

    #[derive(Default)]
    struct App {
        sizes: Vec<(i32, i32)>,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for App {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as wayland_client::Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<XdgWmBase, ()> for App {
        fn event(
            _: &mut Self,
            wm_base: &XdgWmBase,
            event: xdg_wm_base::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = event {
                wm_base.pong(serial);
            }
        }
    }
    impl Dispatch<XdgSurface, ()> for App {
        fn event(
            _: &mut Self,
            surface: &XdgSurface,
            event: xdg_surface::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                surface.ack_configure(serial);
            }
        }
    }
    impl Dispatch<XdgToplevel, ()> for App {
        fn event(
            app: &mut Self,
            _: &XdgToplevel,
            event: xdg_toplevel::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_toplevel::Event::Configure { width, height, .. } = event {
                app.sizes.push((width, height));
            }
        }
    }
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlOutput);
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);
    delegate_noop!(App: ignore wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup);
    delegate_noop!(App: ignore wayland_protocols::xdg::shell::client::xdg_positioner::XdgPositioner);

    use smithay::backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Offscreen,
            damage::OutputDamageTracker,
            element::{Kind, memory::MemoryRenderBufferRenderElement},
            pixman::PixmanRenderer,
        },
    };
    use smithay::utils::{Rectangle, Transform};
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    fn client(stream: UnixStream, count: usize, ready: &mpsc::Sender<()>, stop: &mpsc::Receiver<()>) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let mut app = App::default();
        let mut held = Vec::new();
        for index in 0..count {
            let surface = compositor.create_surface(&handle, ());
            let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
            let toplevel = xdg.get_toplevel(&handle, ());
            toplevel.set_app_id(format!("org.example.Window{index}"));
            toplevel.set_title(format!("Document {index}"));
            surface.commit();
            let _configured = queue.roundtrip(&mut app).unwrap();
            attach(&shm, &surface, &handle, [50, 100, 200, 255], 128);
            let _mapped = queue.roundtrip(&mut app).unwrap();
            if index == 0 {
                let popup_surface = compositor.create_surface(&handle, ());
                let popup_xdg = wm_base.get_xdg_surface(&popup_surface, &handle, ());
                let positioner = wm_base.create_positioner(&handle, ());
                positioner.set_size(20, 20);
                positioner.set_anchor_rect(0, 0, 1, 1);
                positioner.set_offset(30, 30);
                let popup = popup_xdg.get_popup(Some(&xdg), &positioner, &handle, ());
                popup_surface.commit();
                let _popup_configured = queue.roundtrip(&mut app).unwrap();
                attach(&shm, &popup_surface, &handle, [60, 120, 240, 255], 20);
                let _popup_mapped = queue.roundtrip(&mut app).unwrap();
                held.push((popup_surface, popup_xdg, Some(popup), None));
            }
            held.push((surface, xdg, None, Some(toplevel)));
        }
        let _settled = queue.roundtrip(&mut app).unwrap();
        ready.send(()).unwrap();
        stop.recv().unwrap();
        drop(held);
    }

    fn attach(shm: &WlShm, surface: &WlSurface, handle: &QueueHandle<App>, colour: [u8; 4], size: i32) {
        let mut file = tempfile::tempfile().unwrap();
        for _ in 0..size.checked_mul(size).unwrap() {
            file.write_all(&colour).unwrap();
        }
        let pool = shm.create_pool(
            file.as_fd(),
            size.checked_mul(size).unwrap().checked_mul(4).unwrap(),
            handle,
            (),
        );
        let buffer = pool.create_buffer(
            0,
            size,
            size,
            size.checked_mul(4).unwrap(),
            wl_shm::Format::Argb8888,
            handle,
            (),
        );
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
    }

    fn scene(count: usize, check: impl FnOnce(&mut Compositor, &Output)) {
        scene_scaled(count, 1, check);
    }

    fn scene_scaled(count: usize, scale: i32, check: impl FnOnce(&mut Compositor, &Output)) {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Example".into(),
                model: "Test".into(),
            },
        );
        let _global = output.create_global::<Compositor>(&display.handle());
        scale::apply(
            &output,
            Mode {
                size: (800, 600).into(),
                refresh: 60_000,
            },
            scale,
        );
        compositor.space.map_output(&output, (0, 0));
        compositor.output = Some(output.clone());
        compositor.fill_output();
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (ready, received) = mpsc::channel();
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, count, &ready, &stopped));
        let started = Instant::now();
        while received.try_recv().is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            compositor.space.refresh();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        tick(&mut compositor, 0);
        check(&mut compositor, &output);
        stop.send(()).unwrap();
        thread.join().unwrap();
    }

    fn tap(compositor: &mut Compositor) {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            compositor.handle(&Event::Button {
                name: ButtonName::Guide,
                state,
            });
        }
    }
    fn tick(compositor: &mut Compositor, millis: u64) {
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(millis),
        });
    }

    struct Capture {
        renderer: PixmanRenderer,
        damage: OutputDamageTracker,
        cache: std::sync::Mutex<tehuti::wallpaper::WallpaperCache>,
    }
    impl Capture {
        fn new() -> Self {
            Self {
                renderer: PixmanRenderer::new().unwrap(),
                damage: OutputDamageTracker::new((800, 600), 1.0, Transform::Normal),
                cache: std::sync::Mutex::new(tehuti::wallpaper::WallpaperCache::new(
                    "/nonexistent/wallpaper.png".into(),
                )),
            }
        }
        fn expect(&mut self, compositor: &Compositor, output: &Output, point: (u32, u32), expected: [u8; 4]) {
            assert_colour(pixel(&self.pixels(compositor, output), point), expected);
        }
        fn pixels(&mut self, compositor: &Compositor, output: &Output) -> image::RgbaImage {
            let mut buffer = self
                .renderer
                .create_buffer(Fourcc::Abgr8888, (800, 600).into())
                .unwrap();
            let mut target = self.renderer.bind(&mut buffer).unwrap();
            let notice = tehuti::notice::notice_buffer("notice");
            let mut overlays = vec![
                MemoryRenderBufferRenderElement::from_buffer(
                    &mut self.renderer,
                    (16.0, 200.0),
                    &notice,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )
                .unwrap(),
            ];
            let mut bar = tehuti::bar::Buffer::default();
            overlays.push(
                bar.element(&mut self.renderer, output, &compositor.voice_bar(), "12:00")
                    .unwrap()
                    .unwrap(),
            );
            tehuti::wallpaper::render_output_with_cache(
                output,
                &mut self.renderer,
                &mut target,
                compositor,
                &overlays,
                &mut self.damage,
                &self.cache,
            )
            .unwrap()
            .sync
            .wait()
            .unwrap();
            let mapping = self
                .renderer
                .copy_framebuffer(&target, Rectangle::from_size((800, 600).into()), Fourcc::Abgr8888)
                .unwrap();
            image::RgbaImage::from_raw(800, 600, self.renderer.map_texture(&mapping).unwrap().to_vec())
                .unwrap()
        }
    }
    fn window_points(compositor: &Compositor) -> Vec<(u32, u32)> {
        compositor
            .space
            .elements()
            .map(|window| {
                let location = compositor.space.element_location(window).unwrap();
                (
                    u32::try_from(location.x.checked_add(100).unwrap()).unwrap(),
                    u32::try_from(location.y.checked_add(100).unwrap()).unwrap(),
                )
            })
            .collect()
    }
    fn pixel(image: &image::RgbaImage, point: (u32, u32)) -> [u8; 4] {
        image.get_pixel(point.0, point.1).0
    }
    fn assert_colour(actual: [u8; 4], expected: [u8; 4]) {
        for (channel, target) in actual.into_iter().zip(expected) {
            assert!(channel.abs_diff(target) <= 1, "{channel} differs from {target}");
        }
    }

    #[test]
    fn choice_list_shows_wallpaper_and_closing_restores_the_application() {
        scene(1, |compositor, output| {
            let mut capture = Capture::new();
            let point = window_points(compositor)[0];
            let before = pixel(&capture.pixels(compositor, output), point);
            assert_ne!(before, [0, 0, 0, 255]);
            compositor.open_choice_list(tehuti_core::choice_list::Slots {
                title: "Choose".into(),
                ..tehuti_core::choice_list::Slots::default()
            });
            capture.expect(compositor, output, point, [0, 0, 0, 255]);
            let mut pad = tehuti::input::SyntheticPad::default();
            for state in [ButtonState::Pressed, ButtonState::Released] {
                pad.push(Event::Button {
                    name: ButtonName::East,
                    state,
                });
            }
            tehuti::input::route(compositor, &mut pad).unwrap();
            capture.expect(compositor, output, point, before);
            assert_eq!(compositor.space.elements().count(), 1);
        });
    }

    #[test]
    fn focus_outline_is_two_logical_pixels_on_only_the_focused_window_at_both_levels() {
        for scale in [1, 2] {
            scene_scaled(2, scale, |compositor, output| {
                let mut capture = Capture::new();
                capture.damage = OutputDamageTracker::new((800, 600), f64::from(scale), Transform::Normal);
                for _ in 0..2 {
                    let image = capture.pixels(compositor, output);
                    let windows = compositor.space.elements().collect::<Vec<_>>();
                    for (index, window) in windows.iter().enumerate() {
                        let location = compositor.space.element_location(window).unwrap();
                        let bounds = Rectangle::new(location, window.layout_size())
                            .to_physical_precise_round::<f64, i32>(f64::from(scale));
                        let x = u32::try_from(bounds.loc.x).unwrap();
                        let y = u32::try_from(bounds.loc.y).unwrap();
                        let right = x + u32::try_from(bounds.size.w).unwrap() - 1;
                        let bottom = y + u32::try_from(bounds.size.h).unwrap() - 1;
                        let width = u32::try_from(2 * scale).unwrap();
                        let expected = if index == 1 { [255; 4] } else { [134, 67, 34, 255] };
                        for offset in 0..width {
                            if index == 1 {
                                assert_colour(pixel(&image, (x + offset, y + 10)), expected);
                                assert_colour(pixel(&image, (x + 10, y + offset)), expected);
                                assert_colour(pixel(&image, (right - offset, y + 10)), expected);
                                assert_colour(pixel(&image, (x + 10, bottom - offset)), expected);
                            } else {
                                assert_ne!(pixel(&image, (x + offset, y + 10)), [255; 4]);
                            }
                        }
                        assert_ne!(pixel(&image, (x + width, y + 10)), [255; 4]);
                    }
                    tap(compositor);
                }
            });
        }
    }

    #[test]
    fn shoulders_transfer_the_title_highlight_and_outline_at_columns_level() {
        use tehuti_core::bar::{Segment, Tone};
        scene(2, |compositor, output| {
            tap(compositor);
            let mut capture = Capture::new();
            for focused in [1, 0] {
                assert_eq!(
                    compositor.voice_bar().segments(),
                    vec![
                        Segment {
                            text: "org.example.Window0".into(),
                            tone: if focused == 0 {
                                Tone::Context
                            } else {
                                Tone::Unfocused
                            }
                        },
                        Segment {
                            text: "org.example.Window1".into(),
                            tone: if focused == 1 {
                                Tone::Context
                            } else {
                                Tone::Unfocused
                            }
                        },
                    ]
                );
                let image = capture.pixels(compositor, output);
                for (index, window) in compositor.space.elements().enumerate() {
                    let location = compositor.space.element_location(window).unwrap();
                    let point = (
                        u32::try_from(location.x + 1).unwrap(),
                        u32::try_from(location.y + 10).unwrap(),
                    );
                    assert_eq!(pixel(&image, point) == [255; 4], index == focused);
                }
                for state in [ButtonState::Pressed, ButtonState::Released] {
                    compositor.handle(&Event::Button {
                        name: ButtonName::LeftBumper,
                        state,
                    });
                }
            }
        });
    }

    #[test]
    fn context_labels_follow_windows_and_reload_at_both_levels() {
        use tehuti_core::bar::Tone;
        scene(2, |compositor, _| {
            let contexts = || {
                vec![
                    tehuti_protocol::parse_context(
                        "app_id = 'org.example.Window1'\nlabel = 'Editor'\nbuttons = {}",
                    )
                    .unwrap(),
                ]
            };
            compositor.set_contexts(contexts());
            compositor.handle(&Event::Focus {
                app_id: Some("org.example.Window1".into()),
                title: "Document 1".into(),
            });
            assert_eq!(compositor.voice_bar().segments()[0].text, "Editor");
            tap(compositor);
            let bar = compositor.voice_bar();
            assert_eq!(bar.focused_window_segment(), Some(1));
            assert_eq!(
                bar.segments()
                    .iter()
                    .map(|segment| (segment.text.as_str(), segment.tone))
                    .collect::<Vec<_>>(),
                vec![
                    ("org.example.Window0", Tone::Unfocused),
                    ("Editor", Tone::Context)
                ],
            );
            compositor.reload_contexts(Ok(Vec::new()));
            assert_eq!(compositor.voice_bar().segments()[1].text, "org.example.Window1");
            compositor.reload_contexts(Ok(contexts()));
            assert_eq!(compositor.voice_bar().segments()[1].text, "Editor");
            tap(compositor);
            assert_eq!(compositor.voice_bar().segments()[0].text, "Editor");
            compositor.reload_contexts(Ok(Vec::new()));
            assert_eq!(compositor.voice_bar().segments()[0].text, "org.example.Window1");
        });
    }

    #[test]
    fn columns_chips_follow_layout_order_after_a_column_moves() {
        use tehuti_core::bar::Tone;
        scene(3, |compositor, _| {
            compositor.set_contexts(vec![
                tehuti_protocol::parse_context(
                    "app_id = '*'\n[buttons]\nX = { layout = 'move-column-left' }",
                )
                .unwrap(),
            ]);
            tap(compositor);
            for state in [ButtonState::Pressed, ButtonState::Released] {
                compositor.handle(&Event::Button {
                    name: ButtonName::West,
                    state,
                });
            }
            let segments = compositor.voice_bar().segments();
            let windows: Vec<_> = segments
                .iter()
                .filter(|segment| matches!(segment.tone, Tone::Context | Tone::Unfocused))
                .map(|segment| (segment.text.as_str(), segment.tone))
                .collect();
            assert_eq!(
                windows,
                vec![
                    ("org.example.Window0", Tone::Unfocused),
                    ("org.example.Window2", Tone::Context),
                    ("org.example.Window1", Tone::Unfocused),
                ]
            );
            tap(compositor);
            assert_eq!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .filter(|segment| matches!(segment.tone, Tone::Context | Tone::Unfocused))
                    .count(),
                2
            );
        });
    }

    #[test]
    fn stacked_windows_stay_grouped_in_column_order() {
        use tehuti_core::{
            bar::{Segment, Tone},
            layout::Command,
        };
        scene(4, |compositor, _| {
            compositor.apply_layout_command(Command::FocusColumnLeft);
            compositor.apply_layout_command(Command::FocusColumnLeft);
            compositor.apply_layout_command(Command::ConsumeWindowIntoColumn);
            compositor.apply_layout_command(Command::MoveColumnRight);
            tap(compositor);
            assert_eq!(
                compositor.voice_bar().segments(),
                vec![
                    Segment {
                        text: "org.example.Window0".into(),
                        tone: Tone::Unfocused
                    },
                    Segment {
                        text: "org.example.Window3".into(),
                        tone: Tone::Unfocused
                    },
                    Segment {
                        text: "org.example.Window1".into(),
                        tone: Tone::Context
                    },
                    Segment {
                        text: "org.example.Window2".into(),
                        tone: Tone::Unfocused
                    },
                ]
            );
        });
    }

    #[test]
    fn app_level_adds_the_title_and_columns_show_only_short_names() {
        scene(2, |compositor, _| {
            for _ in 0..2 {
                assert_eq!(
                    compositor
                        .voice_bar()
                        .segments()
                        .iter()
                        .map(|segment| segment.text.as_str())
                        .collect::<Vec<_>>(),
                    vec!["org.example.Window1", "Document 1"]
                );
                tap(compositor);
                assert_eq!(
                    compositor
                        .voice_bar()
                        .segments()
                        .iter()
                        .map(|segment| segment.text.as_str())
                        .collect::<Vec<_>>(),
                    vec!["org.example.Window0", "org.example.Window1"]
                );
                tap(compositor);
            }
        });
    }

    #[test]
    fn no_focus_has_no_outline() {
        scene(0, |compositor, output| {
            for _ in 0..2 {
                assert!(compositor.focus_outline(output).is_empty());
                tap(compositor);
            }
        });
    }

    #[test]
    fn guide_to_app_level_fades_other_windows_and_popups_but_keeps_overlays_bright() {
        scene(2, |compositor, output| {
            tap(compositor);
            let points = window_points(compositor);
            let mut capture = Capture::new();
            let baseline = capture.pixels(compositor, output);
            for point in &points {
                assert_colour(pixel(&baseline, *point), [200, 100, 50, 255]);
            }
            tap(compositor);
            let initial = capture.pixels(compositor, output);
            tick(compositor, 50);
            let halfway = capture.pixels(compositor, output);
            tick(compositor, 100);
            let dimmed = capture.pixels(compositor, output);
            assert_colour(pixel(&dimmed, points[0]), [134, 67, 34, 255]);
            assert_eq!(pixel(&dimmed, points[1]), pixel(&baseline, points[1]));
            assert!(pixel(&initial, points[0])[0] > pixel(&halfway, points[0])[0]);
            assert!(pixel(&halfway, points[0])[0] > pixel(&dimmed, points[0])[0]);
            let popup = (points[0].0 - 70, points[0].1 - 70);
            assert_colour(pixel(&baseline, popup), [240, 120, 60, 255]);
            assert_colour(pixel(&dimmed, popup), [161, 80, 40, 255]);
            for point in [(16, 200), (799, 599)] {
                assert_eq!(pixel(&dimmed, point), pixel(&baseline, point));
            }
            tick(compositor, 10_000);
            capture.expect(compositor, output, points[0], [134, 67, 34, 255]);
            tap(compositor);
            let columns = capture.pixels(compositor, output);
            for point in &points {
                assert_eq!(pixel(&columns, *point), pixel(&baseline, *point));
            }
        });
    }

    #[test]
    fn unfocused_fade_preserves_linear_brightness_at_each_quarter() {
        scene(2, |compositor, output| {
            let directory = tempfile::tempdir().unwrap();
            std::fs::create_dir(directory.path().join("global")).unwrap();
            std::fs::write(
                directory.path().join("global/context.toml"),
                "app_id = '*'\n[buttons]\n[dim]\nstrength = 0.2\nfade_ms = 200\n",
            )
            .unwrap();
            compositor.reload_contexts(tehuti::contexts::try_load(directory.path()));
            tap(compositor);
            tap(compositor);
            let points = window_points(compositor);
            let mut capture = Capture::new();
            capture.expect(compositor, output, points[0], [200, 100, 50, 255]);
            for expected in [
                [160, 80, 40, 255],
                [120, 60, 30, 255],
                [80, 40, 20, 255],
                [40, 20, 10, 255],
            ] {
                tick(compositor, 50);
                capture.expect(compositor, output, points[0], expected);
                capture.expect(compositor, output, points[1], [200, 100, 50, 255]);
            }
        });
    }

    #[test]
    fn one_visible_window_including_fullscreen_stays_bright() {
        scene(2, |compositor, output| {
            tick(compositor, 150);
            let points = window_points(compositor);
            assert_colour(
                pixel(&Capture::new().pixels(compositor, output), points[0]),
                [134, 67, 34, 255],
            );
        });
        scene(1, |compositor, output| {
            let mut capture = Capture::new();
            tap(compositor);
            tap(compositor);
            tick(compositor, 150);
            let point = window_points(compositor)[0];
            capture.expect(compositor, output, point, [200, 100, 50, 255]);
            compositor.set_contexts(vec![
                tehuti_protocol::parse_context(
                    "app_id = '*'\n[buttons]\nX = { layout = 'toggle-fullscreen' }",
                )
                .unwrap(),
            ]);
            for state in [ButtonState::Pressed, ButtonState::Released] {
                compositor.handle(&Event::Button {
                    name: ButtonName::West,
                    state,
                });
            }
            tick(compositor, 150);
            assert!(
                compositor
                    .layout_snapshot()
                    .outputs
                    .iter()
                    .flat_map(|snapshot_output| &snapshot_output.workspaces)
                    .flat_map(|workspace| &workspace.columns)
                    .flat_map(|column| &column.tiles)
                    .any(|tile| tile.mode == tehuti_core::layout::TileMode::Fullscreen)
            );
            assert!(compositor.focus_outline(output).is_empty());
            let fullscreen_point = window_points(compositor)[0];
            capture.expect(compositor, output, fullscreen_point, [200, 100, 50, 255]);
        });
    }

    #[test]
    fn toml_strength_hot_reloads_and_context_override_can_disable_dimming() {
        scene(2, |compositor, output| {
            let directory = tempfile::tempdir().unwrap();
            std::fs::create_dir(directory.path().join("global")).unwrap();
            let path = directory.path().join("global/context.toml");
            std::fs::write(&path, "app_id = '*'\n[buttons]\n[dim]\nstrength = 0.5\n").unwrap();
            compositor.reload_contexts(tehuti::contexts::try_load(directory.path()));
            tick(compositor, 150);
            let points = window_points(compositor);
            let mut capture = Capture::new();
            capture.expect(compositor, output, points[0], [100, 50, 25, 255]);
            std::fs::write(&path, "app_id = '*'\n[buttons]\n[dim]\nstrength = 0.8\n").unwrap();
            compositor.reload_contexts(tehuti::contexts::try_load(directory.path()));
            capture.expect(compositor, output, points[0], [160, 80, 40, 255]);
            std::fs::write(
                &path,
                "app_id = '*'\n[buttons]\n[dim]\nstrength = 0.8\nfade_ms = 200\n",
            )
            .unwrap();
            compositor.reload_contexts(tehuti::contexts::try_load(directory.path()));
            tap(compositor);
            tap(compositor);
            tick(compositor, 100);
            capture.expect(compositor, output, points[0], [180, 90, 45, 255]);
            tick(compositor, 100);
            capture.expect(compositor, output, points[0], [160, 80, 40, 255]);
            std::fs::create_dir(directory.path().join("app")).unwrap();
            std::fs::write(
                directory.path().join("app/context.toml"),
                "app_id = 'org.example.Window1'\n[buttons]\n[dim]\nenabled = false\n",
            )
            .unwrap();
            compositor.reload_contexts(tehuti::contexts::try_load(directory.path()));
            tick(compositor, 150);
            for point in &points {
                capture.expect(compositor, output, *point, [200, 100, 50, 255]);
            }
            std::fs::remove_file(directory.path().join("app/context.toml")).unwrap();
            std::fs::write(
                &path,
                "app_id = '*'\n[buttons]\n[dim]\nenabled = false\nstrength = 0.8\n",
            )
            .unwrap();
            compositor.reload_contexts(tehuti::contexts::try_load(directory.path()));
            capture.expect(compositor, output, points[0], [200, 100, 50, 255]);
            std::fs::write(
                directory.path().join("app/context.toml"),
                "app_id = 'org.example.Window1'\n[buttons]\n[dim]\nenabled = true\n",
            )
            .unwrap();
            compositor.reload_contexts(tehuti::contexts::try_load(directory.path()));
            capture.expect(compositor, output, points[0], [160, 80, 40, 255]);
        });
    }
}
