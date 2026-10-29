//! Clients receive their assigned layout size before drawing their first buffer.

#[cfg(test)]
mod initial_size {
    use core::time::Duration;
    use std::{
        os::{fd::AsFd, unix::net::UnixStream},
        sync::{Arc, mpsc},
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

    fn client(
        stream: UnixStream,
        app_id: Option<&str>,
        inherited: bool,
        ready: &mpsc::Sender<()>,
        release: &mpsc::Receiver<()>,
    ) -> Vec<(i32, i32)> {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let _output: WlOutput = globals.bind(&handle, 1..=4, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg_surface.get_toplevel(&handle, ());
        if let Some(app_id) = app_id {
            toplevel.set_app_id(app_id.into());
        }
        if inherited {
            let parent = compositor.create_surface(&handle, ());
            let parent_xdg = wm_base.get_xdg_surface(&parent, &handle, ());
            let parent_toplevel = parent_xdg.get_toplevel(&handle, ());
            parent_toplevel.set_app_id("org.example.Launcher".into());
            toplevel.set_parent(Some(&parent_toplevel));
        }
        surface.commit();
        let mut app = App::default();
        let _first = queue.roundtrip(&mut app).unwrap();
        let _second = queue.roundtrip(&mut app).unwrap();
        surface.commit();
        let _unmapped = queue.roundtrip(&mut app).unwrap();
        assert!(
            app.sizes
                .first()
                .is_some_and(|&(width, height)| width > 0 && height > 0)
        );
        assert!(
            app.sizes.iter().all(|size| Some(size) == app.sizes.first()),
            "{:?}",
            app.sizes
        );
        let file = tempfile::tempfile().unwrap();
        file.set_len(64 * 64 * 4).unwrap();
        let pool = shm.create_pool(file.as_fd(), 64 * 64 * 4, &handle, ());
        let buffer = pool.create_buffer(0, 64, 64, 256, wl_shm::Format::Argb8888, &handle, ());
        xdg_surface.set_window_geometry(8, 12, 40, 32);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        let _third = queue.roundtrip(&mut app).unwrap();
        let _fourth = queue.roundtrip(&mut app).unwrap();
        ready.send(()).unwrap();
        release.recv().unwrap();
        app.sizes
    }

    #[test]
    fn mpv_gets_layout_size_in_the_first_configure() {
        for scale in [1, 2] {
            check_initial_size(Some("mpv"), scale);
        }
    }

    #[test]
    fn other_clients_get_layout_size_in_the_first_configure() {
        for scale in [1, 2] {
            check_initial_size(Some("org.example.Video"), scale);
        }
    }

    #[test]
    fn clients_without_an_app_id_get_layout_size_in_the_first_configure() {
        for scale in [1, 2] {
            check_initial_size(None, scale);
        }
    }

    #[test]
    fn context_sizes_reach_the_first_configure_directly_and_through_parents() {
        for scale in [1, 2] {
            for inherited in [false, true] {
                check_size(
                    Some("org.example.Video"),
                    scale,
                    Some(if inherited {
                        "org.example.Launcher"
                    } else {
                        "org.example.Video"
                    }),
                    inherited,
                );
            }
        }
    }

    fn check_initial_size(app_id: Option<&'static str>, scale: i32) {
        check_size(app_id, scale, None, false);
    }

    fn add_output(compositor: &mut Compositor, display: &Display<Compositor>, scale: i32) {
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        let _global = output.create_global::<Compositor>(&display.handle());
        scale::apply(
            &output,
            Mode {
                size: (3840, 2160).into(),
                refresh: 60_000,
            },
            scale,
        );
        compositor.space.map_output(&output, (0, 0));
        compositor.output = Some(output);
        compositor.fill_output();
    }

    fn check_size(app_id: Option<&'static str>, scale: i32, context_id: Option<&str>, inherited: bool) {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        if let Some(context_id) = context_id {
            compositor.set_contexts(vec![tehuti_protocol::parse_context(&format!("app_id = '{context_id}'\nbuttons = {{}}\n[window]\ncolumn_width = 'one-half'\nheight = 0.75")).unwrap()]);
        }
        add_output(&mut compositor, &display, scale);
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (ready, mapped) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, app_id, inherited, &ready, &wait));
        let started = Instant::now();
        while mapped.try_recv().is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            compositor.space.refresh();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        assert_committed_geometry(&compositor, scale);
        release.send(()).unwrap();
        let sizes = thread.join().unwrap();
        let snapshot = compositor.layout_snapshot();
        let snapshot_output = &snapshot.outputs[0];
        let column = &snapshot_output.workspaces[snapshot_output.active_workspace].columns[0];
        if context_id.is_some() {
            assert_eq!(
                column.width,
                Some(tehuti_core::layout::PresetSize::Proportion(0.5))
            );
            assert_eq!(
                column.tiles[0].height,
                tehuti_core::layout::WindowHeight::Proportion(0.75)
            );
        }
        let requested = column.tiles[0].requested_client;
        assert_eq!(
            sizes.first(),
            Some(&{
                let size = smithay::utils::Size::<f64, smithay::utils::Logical>::from((
                    requested.width,
                    requested.height,
                ))
                .to_i32_round();
                (size.w, size.h)
            }),
            "{sizes:?}"
        );
        assert!(sizes.iter().all(|size| Some(size) == sizes.first()), "{sizes:?}");
    }
    #[expect(
        clippy::float_arithmetic,
        reason = "compare committed geometry centres in logical pixels"
    )]
    fn assert_committed_geometry(compositor: &Compositor, scale: i32) {
        use smithay::backend::renderer::element::Element;
        let window = compositor.space.elements().next().unwrap();
        let placed = compositor.space.element_geometry(window).unwrap();
        let snapshot = compositor.layout_snapshot();
        let snapshot_output = &snapshot.outputs[0];
        let column = &snapshot_output.workspaces[snapshot_output.active_workspace].columns[0];
        let tile = column.tiles[0].tile;
        assert_eq!(placed.size, (40, 32).into());
        let centre: smithay::utils::Point<i32, smithay::utils::Logical> =
            smithay::utils::Point::<f64, smithay::utils::Logical>::from((
                tile.x + tile.width / 2.0,
                tile.y + tile.height / 2.0,
            ))
            .to_i32_round();
        assert!(placed.loc.x.saturating_add(20).abs_diff(centre.x) <= 1);
        assert!(placed.loc.y.saturating_add(16).abs_diff(centre.y) <= 1);
        let edges = compositor.focus_outline(compositor.output.as_ref().unwrap());
        assert_eq!(edges.len(), 4);
        let bounds = edges
            .iter()
            .map(|edge| edge.geometry(f64::from(scale).into()))
            .reduce(smithay::utils::Rectangle::merge)
            .unwrap();
        assert_eq!(bounds, placed.to_physical_precise_round(f64::from(scale)));
    }
}
