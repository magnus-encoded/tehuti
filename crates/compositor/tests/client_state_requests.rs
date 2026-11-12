//! Tiled clients hear they are tiled, and their maximize and fullscreen requests get answers.

#[cfg(test)]
mod client_state_requests {
    use core::time::Duration;
    use std::{
        io::Write,
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

    const OUTPUT_WIDTH: i32 = 1920;
    const OUTPUT_HEIGHT: i32 = 1080;

    #[derive(Default)]
    struct App {
        /// Every toplevel configure as (width, height, states).
        configures: Vec<(i32, i32, Vec<u32>)>,
        acked: bool,
        /// Re-asks for maximize on every configure before acknowledging it, as
        /// a Wine game through xwayland-satellite does.
        echo_maximize: Option<XdgToplevel>,
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
            app: &mut Self,
            surface: &XdgSurface,
            event: xdg_surface::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                if let Some(toplevel) = &app.echo_maximize {
                    toplevel.set_maximized();
                }
                surface.ack_configure(serial);
                app.acked = true;
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
            {
                let states = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_ne_bytes(*bytes))
                    .collect();
                app.configures.push((width, height, states));
            }
        }
    }
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlOutput);
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);

    /// Draws a buffer of the given size.
    fn draw(shm: &WlShm, surface: &WlSurface, handle: &QueueHandle<App>, size: (i32, i32)) {
        let (width, height) = (size.0.max(1), size.1.max(1));
        let bytes = width.saturating_mul(height).saturating_mul(4);
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![255; usize::try_from(bytes).unwrap()])
            .unwrap();
        let pool = shm.create_pool(file.as_fd(), bytes, handle, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width.saturating_mul(4),
            wl_shm::Format::Argb8888,
            handle,
            (),
        );
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
    }

    /// What the client heard in each step of its script.
    #[derive(Debug, Default)]
    struct Report {
        first: Vec<u32>,
        first_size: (i32, i32),
        first_count: usize,
        after_maximize: usize,
        echoed: usize,
        after_unmaximize: usize,
        fullscreen: Option<Vec<u32>>,
        unfullscreen: Option<Vec<u32>>,
        unfullscreen_size: Option<(i32, i32)>,
        windowed_configures: usize,
    }

    /// Roundtrips until the compositor goes quiet, drawing every acknowledged configure.
    fn settle(queue: &mut wayland_client::EventQueue<App>, app: &mut App, shm: &WlShm, surface: &WlSurface) {
        let mut quiet = 0_u32;
        while quiet < 5 {
            let before = app.configures.len();
            let _dispatched = queue.roundtrip(app).unwrap();
            if core::mem::take(&mut app.acked) {
                let &(width, height, _) = app.configures.last().unwrap();
                draw(shm, surface, &queue.handle(), (width, height));
            }
            quiet = if app.configures.len() == before {
                quiet.saturating_add(1)
            } else {
                0
            };
        }
    }

    fn client(stream: UnixStream, early_fullscreen: bool, windowed_size: bool) -> Report {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 2..=6, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let mut app = App::default();
        let surface = compositor.create_surface(&handle, ());
        let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg.get_toplevel(&handle, ());
        toplevel.set_title("Test window".into());
        toplevel.set_app_id("org.example.Game".into());
        if early_fullscreen {
            toplevel.set_fullscreen(None);
        }
        surface.commit();
        settle(&mut queue, &mut app, &shm, &surface);
        let mut report = Report {
            first: app.configures.last().unwrap().2.clone(),
            first_size: (app.configures[0].0, app.configures[0].1),
            first_count: app.configures.len(),
            ..Report::default()
        };
        if early_fullscreen {
            toplevel.unset_fullscreen();
            settle(&mut queue, &mut app, &shm, &surface);
            report.unfullscreen = app.configures.last().map(|configure| configure.2.clone());
            report.unfullscreen_size = app.configures.last().map(|configure| (configure.0, configure.1));
            return report;
        }
        if windowed_size {
            let count = app.configures.len();
            draw(&shm, &surface, &handle, (800, 600));
            settle(&mut queue, &mut app, &shm, &surface);
            report.windowed_configures = app.configures.len().saturating_sub(count);
            return report;
        }
        exercise_state_requests(
            &mut queue,
            &mut app,
            &shm,
            &surface,
            &handle,
            &toplevel,
            &mut report,
        );
        report
    }

    fn exercise_state_requests(
        queue: &mut wayland_client::EventQueue<App>,
        app: &mut App,
        shm: &WlShm,
        surface: &WlSurface,
        handle: &QueueHandle<App>,
        toplevel: &XdgToplevel,
        report: &mut Report,
    ) {
        let mut count = app.configures.len();
        toplevel.set_maximized();
        settle(queue, app, shm, surface);
        report.after_maximize = app.configures.len().saturating_sub(count);
        count = app.configures.len();
        app.echo_maximize = Some(toplevel.clone());
        toplevel.set_maximized();
        for _ in 0..60 {
            let _dispatched = queue.roundtrip(app).unwrap();
            if core::mem::take(&mut app.acked) {
                draw(shm, surface, handle, (OUTPUT_WIDTH, OUTPUT_HEIGHT));
            }
        }
        app.echo_maximize = None;
        settle(queue, app, shm, surface);
        report.echoed = app.configures.len().saturating_sub(count);
        count = app.configures.len();
        toplevel.unset_maximized();
        settle(queue, app, shm, surface);
        report.after_unmaximize = app.configures.len().saturating_sub(count);
        toplevel.set_fullscreen(None);
        settle(queue, app, shm, surface);
        report.fullscreen = app.configures.last().map(|configure| configure.2.clone());
        toplevel.unset_fullscreen();
        settle(queue, app, shm, surface);
        report.unfullscreen = app.configures.last().map(|configure| configure.2.clone());
        report.unfullscreen_size = app.configures.last().map(|configure| (configure.0, configure.1));
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

    fn run(early_fullscreen: bool, windowed_size: bool) -> Report {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        attach_output(&display, &mut compositor);
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let thread = thread::spawn(move || client(stream, early_fullscreen, windowed_size));
        let started = Instant::now();
        while !thread.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(20), "client timed out");
            pump(&mut display, &mut compositor, Duration::from_millis(1));
        }
        thread.join().unwrap()
    }

    #[test]
    fn tiled_clients_hear_they_are_tiled_and_get_answers() {
        let report = run(false, false);
        let fullscreen_state = u32::from(xdg_toplevel::State::Fullscreen);
        let tiled = [
            xdg_toplevel::State::TiledLeft,
            xdg_toplevel::State::TiledRight,
            xdg_toplevel::State::TiledTop,
            xdg_toplevel::State::TiledBottom,
        ]
        .map(u32::from);
        for state in tiled {
            assert!(report.first.contains(&state), "{report:?}");
        }
        assert_eq!(report.after_maximize, 1, "maximize gets exactly one configure");
        assert!(
            report.echoed <= 2,
            "a client that re-asks after every configure and commits the output size settles: {report:?}"
        );
        assert_eq!(
            report.after_unmaximize, 1,
            "unmaximize gets exactly one configure"
        );
        let fullscreen = report.fullscreen.unwrap();
        assert!(fullscreen.contains(&fullscreen_state), "{fullscreen:?}");
        assert!(!fullscreen.contains(&tiled[0]), "{fullscreen:?}");
        let unfullscreen = report.unfullscreen.unwrap();
        assert!(!unfullscreen.contains(&fullscreen_state), "{unfullscreen:?}");
        assert!(unfullscreen.contains(&tiled[0]), "{unfullscreen:?}");
    }

    #[test]
    fn windowed_client_drawing_its_requested_size_gets_stable_configure() {
        let report = run(false, true);
        assert_eq!(report.first_count, 1, "{report:?}");
        assert_eq!(report.windowed_configures, 0, "{report:?}");
    }

    #[test]
    fn fullscreen_before_first_commit_configures_output_once_and_restores_tile() {
        let report = run(true, false);
        assert_eq!(report.first_count, 1, "{report:?}");
        assert_eq!(
            report.first_size,
            (OUTPUT_WIDTH, OUTPUT_HEIGHT - tehuti_core::bar::HEIGHT),
            "fullscreen leaves the bar exclusive zone clear: {report:?}"
        );
        assert!(report.first.contains(&u32::from(xdg_toplevel::State::Fullscreen)));
        let tile_size = report.unfullscreen_size.unwrap();
        assert!(
            tile_size.0 < OUTPUT_WIDTH && tile_size.1 < OUTPUT_HEIGHT,
            "{report:?}"
        );
        assert!(
            !report
                .unfullscreen
                .unwrap()
                .contains(&u32::from(xdg_toplevel::State::Fullscreen))
        );
    }
}
