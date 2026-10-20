//! Clients choose their first buffer size, then receive the assigned layout size.

#[cfg(test)]
mod initial_size {
    use core::time::Duration;
    use std::{
        os::{fd::AsFd, unix::net::UnixStream},
        sync::Arc,
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

    fn client(stream: UnixStream, app_id: Option<&str>) -> Vec<(i32, i32)> {
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
        surface.commit();
        let mut app = App::default();
        let _first = queue.roundtrip(&mut app).unwrap();
        let _second = queue.roundtrip(&mut app).unwrap();
        surface.commit();
        let _unmapped = queue.roundtrip(&mut app).unwrap();
        assert_eq!(app.sizes.first(), Some(&(0, 0)));
        assert!(app.sizes.iter().all(|size| *size == (0, 0)), "{:?}", app.sizes);
        let file = tempfile::tempfile().unwrap();
        file.set_len(64 * 64 * 4).unwrap();
        let pool = shm.create_pool(file.as_fd(), 64 * 64 * 4, &handle, ());
        let buffer = pool.create_buffer(0, 64, 64, 256, wl_shm::Format::Argb8888, &handle, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        let _third = queue.roundtrip(&mut app).unwrap();
        let _fourth = queue.roundtrip(&mut app).unwrap();
        app.sizes
    }

    #[test]
    fn mpv_gets_no_size_hint_until_it_maps() {
        for scale in [1, 2] {
            check_initial_size(Some("mpv"), scale);
        }
    }

    #[test]
    fn other_clients_get_no_size_hint_until_they_map() {
        for scale in [1, 2] {
            check_initial_size(Some("org.example.Video"), scale);
        }
    }

    #[test]
    fn clients_without_an_app_id_get_no_size_hint_until_they_map() {
        for scale in [1, 2] {
            check_initial_size(None, scale);
        }
    }

    fn check_initial_size(app_id: Option<&'static str>, scale: i32) {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
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
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let thread = thread::spawn(move || client(stream, app_id));
        let started = Instant::now();
        while !thread.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            compositor.space.refresh();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        let sizes = thread.join().unwrap();
        assert_eq!(sizes.first(), Some(&(0, 0)), "{sizes:?}");
        assert!(
            sizes
                .iter()
                .skip(1)
                .any(|&(width, height)| width > 0 && height > 0),
            "{sizes:?}"
        );
    }
}
