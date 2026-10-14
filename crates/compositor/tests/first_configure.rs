//! A new toplevel's first configure already carries its tile size.

#[cfg(test)]
mod first_configure {
    use core::time::Duration;
    use std::{
        os::unix::net::UnixStream,
        sync::{Arc, mpsc},
        thread,
        time::Instant,
    };

    use smithay::reexports::wayland_server::Display;
    use tehuti::state::{ClientState, Compositor};
    use wayland_client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry},
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::{self, XdgToplevel},
        xdg_wm_base::{self, XdgWmBase},
    };

    #[derive(Default)]
    struct App {
        first_size: Option<(i32, i32)>,
        first_tiled_edges: usize,
    }

    delegate_noop!(App: WlCompositor);
    delegate_noop!(App: ignore wayland_client::protocol::wl_surface::WlSurface);

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
            if let xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } = event
                && app.first_size.is_none()
            {
                app.first_size = Some((width, height));
                // States arrive as native-endian u32 values.
                app.first_tiled_edges = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_ne_bytes(*bytes))
                    .filter_map(|state| xdg_toplevel::State::try_from(state).ok())
                    .filter(|state| {
                        matches!(
                            state,
                            xdg_toplevel::State::TiledLeft
                                | xdg_toplevel::State::TiledRight
                                | xdg_toplevel::State::TiledTop
                                | xdg_toplevel::State::TiledBottom
                        )
                    })
                    .count();
            }
        }
    }

    fn first_configured_size(
        client: UnixStream,
        reported: &mpsc::Sender<((i32, i32), usize)>,
        release: &mpsc::Receiver<()>,
    ) {
        let connection = Connection::from_socket(client).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 2..=6, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg_surface.get_toplevel(&handle, ());
        toplevel.set_app_id("mpv".into());
        surface.commit();
        let mut app = App::default();
        let _events = queue.roundtrip(&mut app).unwrap();
        reported
            .send((
                app.first_size.expect("a configure arrived"),
                app.first_tiled_edges,
            ))
            .unwrap();
        let _released = release.recv();
    }

    #[test]
    fn initial_configure_has_the_tile_size() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let (server, client) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (reported, report) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = thread::spawn(move || first_configured_size(client, &reported, &released));
        let began = Instant::now();
        let (size, tiled_edges) = loop {
            assert!(began.elapsed() < Duration::from_secs(5), "client timed out");
            let _events = display.dispatch_clients(&mut compositor).unwrap();
            display.flush_clients().unwrap();
            if let Ok(report) = report.try_recv() {
                break report;
            }
        };
        let snapshot = compositor.layout_snapshot();
        let tile = &snapshot.outputs[0].workspaces[0].columns[0].tiles[0];
        assert!(
            (f64::from(size.0) - tile.requested_client.width).abs() <= 0.5
                && (f64::from(size.1) - tile.requested_client.height).abs() <= 0.5,
            "first configure {size:?} matches the tile {:?}",
            tile.requested_client
        );
        assert!(size.0 > 0 && size.1 > 0);
        assert_eq!(
            tiled_edges, 4,
            "a tiled window is told all four edges are tiled, so clients such as mpv obey the size"
        );
        release.send(()).unwrap();
        worker.join().unwrap();
    }
}
