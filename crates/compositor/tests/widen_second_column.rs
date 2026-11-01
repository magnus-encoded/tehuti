//! Widening the second of two columns draws it at its tile and leaves no gap at the right edge.

#[cfg(test)]
mod widen_second_column {
    use core::{
        ops::{Add as _, Sub as _},
        time::Duration,
    };
    use std::{
        io::Write,
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
    use tehuti_core::layout::Command;
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

    /// Client-side shadow around the window geometry, as decorated toolkits draw.
    const SHADOW: i32 = 20;
    const OUTPUT_WIDTH: i32 = 1920;
    const OUTPUT_HEIGHT: i32 = 1080;

    #[derive(Default)]
    struct App {
        configured: Vec<(i32, i32)>,
        acked: Vec<usize>,
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
    impl Dispatch<XdgSurface, usize> for App {
        fn event(
            app: &mut Self,
            surface: &XdgSurface,
            event: xdg_surface::Event,
            index: &usize,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                surface.ack_configure(serial);
                app.acked.push(*index);
            }
        }
    }
    impl Dispatch<XdgToplevel, usize> for App {
        fn event(
            app: &mut Self,
            _: &XdgToplevel,
            event: xdg_toplevel::Event,
            index: &usize,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_toplevel::Event::Configure { width, height, .. } = event
                && let Some(size) = app.configured.get_mut(*index)
            {
                *size = (
                    if width > 0 { width } else { 300 },
                    if height > 0 { height } else { 300 },
                );
            }
        }
    }
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlOutput);
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "buffer sizes are small test constants"
    )]
    /// Draws a buffer of the configured size plus a shadow on every side.
    fn draw(shm: &WlShm, surface: &WlSurface, xdg: &XdgSurface, handle: &QueueHandle<App>, size: (i32, i32)) {
        let (width, height) = (size.0 + 2 * SHADOW, size.1 + 2 * SHADOW);
        let bytes = width * height * 4;
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![255; usize::try_from(bytes).unwrap()])
            .unwrap();
        let pool = shm.create_pool(file.as_fd(), bytes, handle, ());
        let buffer = pool.create_buffer(0, width, height, width * 4, wl_shm::Format::Argb8888, handle, ());
        xdg.set_window_geometry(SHADOW, SHADOW, size.0, size.1);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
    }

    fn client(stream: UnixStream, count: usize, ready: &mpsc::Sender<()>, stop: &mpsc::Receiver<()>) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let mut app = App::default();
        let mut windows = Vec::new();
        for index in 0..count {
            let surface = compositor.create_surface(&handle, ());
            let xdg = wm_base.get_xdg_surface(&surface, &handle, index);
            let toplevel = xdg.get_toplevel(&handle, index);
            toplevel.set_app_id(format!("org.example.Window{index}"));
            app.configured.push((300, 300));
            surface.commit();
            windows.push((surface, xdg, toplevel));
            let _configured = queue.roundtrip(&mut app).unwrap();
        }
        let mut announced = false;
        while stop.try_recv().is_err() {
            for index in core::mem::take(&mut app.acked) {
                let (surface, xdg, _) = &windows[index];
                draw(&shm, surface, xdg, &handle, app.configured[index]);
            }
            let _dispatched = queue.roundtrip(&mut app).unwrap();
            if !announced && app.acked.is_empty() {
                ready.send(()).unwrap();
                announced = true;
            }
        }
    }

    fn pump(display: &mut Display<Compositor>, compositor: &mut Compositor, duration: Duration) {
        let started = Instant::now();
        while started.elapsed() < duration {
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            compositor.space.refresh();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn app_id(window: &smithay::desktop::Window) -> Option<String> {
        smithay::wayland::compositor::with_states(window.toplevel()?.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|attributes| attributes.lock().ok()?.app_id.clone())
        })
    }

    fn attach_output(display: &Display<Compositor>, compositor: &mut Compositor) {
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
                size: (OUTPUT_WIDTH, OUTPUT_HEIGHT).into(),
                refresh: 60_000,
            },
            1,
        );
        compositor.space.map_output(&output, (0, 0));
        compositor.output = Some(output.clone());
        compositor.fill_output();
    }

    /// Drawn left edge and width of the window with this `app_id`.
    fn drawn(compositor: &Compositor, name: &str) -> (f64, f64) {
        let window = compositor
            .space
            .elements()
            .find(|window| app_id(window).as_deref() == Some(name))
            .unwrap();
        let location = compositor.space.element_location(window).unwrap();
        (f64::from(location.x), f64::from(window.geometry().size.w))
    }

    /// Asserts each column is drawn at its tile and returns the drawn left and right edges
    /// with the output gaps.
    fn drawn_columns(compositor: &Compositor) -> (Vec<(f64, f64)>, f64) {
        let snapshot = compositor.layout_snapshot();
        let output = &snapshot.outputs[0];
        let workspace = &output.workspaces[output.active_workspace];
        assert_eq!(workspace.focused_column, Some(1));
        let mut edges = Vec::new();
        for (index, column) in workspace.columns.iter().enumerate() {
            let tile = &column.tiles[column.focused_tile];
            let (left, width) = drawn(compositor, &format!("org.example.Window{index}"));
            let expected = tile.place_client(
                tehuti_core::layout::Size {
                    width,
                    height: tile.requested_client.height,
                },
                output.border,
            );
            assert_eq!(
                width,
                tile.requested_client.width.round(),
                "column {index} drew at its tile width"
            );
            assert!(
                left.sub(expected.x).abs() <= 1.0,
                "column {index} drawn at {left}, tile at {}",
                expected.x
            );
            edges.push((left, left.add(width)));
        }
        (edges, output.gaps)
    }

    #[test]
    fn widening_the_second_of_two_columns_leaves_no_gap_and_draws_the_tile() {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        attach_output(&display, &mut compositor);
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (ready, received) = mpsc::channel();
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, 2, &ready, &stopped));
        let started = Instant::now();
        while received.try_recv().is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            pump(&mut display, &mut compositor, Duration::from_millis(1));
        }
        pump(&mut display, &mut compositor, Duration::from_millis(200));
        for _ in 0..4 {
            compositor.apply_layout_command(Command::SwitchPresetColumnWidth);
            pump(&mut display, &mut compositor, Duration::from_millis(200));
            let (edges, gaps) = drawn_columns(&compositor);
            let strip_starts_left = edges[0].0 <= gaps.add(1.0);
            let strip_reaches_right = edges[1].1 >= f64::from(OUTPUT_WIDTH).sub(gaps).sub(1.0);
            assert!(
                strip_starts_left || strip_reaches_right,
                "gap past the last column: {edges:?}"
            );
            assert!(
                edges[1].0 >= 0.0 && edges[1].1 <= f64::from(OUTPUT_WIDTH),
                "{edges:?}"
            );
        }
        stop.send(()).unwrap();
        while !thread.is_finished() {
            pump(&mut display, &mut compositor, Duration::from_millis(1));
        }
        thread.join().unwrap();
    }
}
