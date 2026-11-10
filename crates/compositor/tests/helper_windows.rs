//! Untitled small clients do not consume columns or steal focus.

#[cfg(test)]
mod helper_windows {
    use core::time::Duration;
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
    use wayland_client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_buffer::WlBuffer,
            wl_compositor::WlCompositor,
            wl_keyboard::{self, WlKeyboard},
            wl_output::WlOutput,
            wl_registry::WlRegistry,
            wl_seat::WlSeat,
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
        focused: Option<WlSurface>,
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
    impl Dispatch<WlKeyboard, ()> for App {
        fn event(
            app: &mut Self,
            _: &WlKeyboard,
            event: wl_keyboard::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_keyboard::Event::Enter { surface, .. } = event {
                app.focused = Some(surface);
            } else if let wl_keyboard::Event::Leave { .. } = event {
                app.focused = None;
            }
        }
    }
    delegate_noop!(App: ignore WlSeat);
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

    fn advance_helper(
        stage: usize,
        helper_top: &XdgToplevel,
        helper: &WlSurface,
        shm: &WlShm,
        handle: &QueueHandle<App>,
    ) {
        match stage {
            1 => helper.commit(),
            2 | 8 => draw(shm, helper, handle, (26, 26)),
            3 => helper_top.set_title("Tools".into()),
            4 => helper_top.set_title(String::new()),
            5 => draw(shm, helper, handle, (64, 64)),
            6 => draw(shm, helper, handle, (65, 26)),
            7 => draw(shm, helper, handle, (26, 65)),
            9 => {
                helper_top.set_title("Tools".into());
                helper.attach(None, 0, 0);
                helper.commit();
            }
            _ => {}
        }
    }

    fn client(stream: UnixStream, ready: &mpsc::Sender<usize>, commands: &mpsc::Receiver<()>) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 2..=6, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let seat: WlSeat = globals.bind(&handle, 1..=5, ()).unwrap();
        let _keyboard = seat.get_keyboard(&handle, ());
        let mut app = App::default();
        let game = compositor.create_surface(&handle, ());
        let game_xdg = wm_base.get_xdg_surface(&game, &handle, ());
        let game_top = game_xdg.get_toplevel(&handle, ());
        game_top.set_title("Game".into());
        game.commit();
        let _configured = queue.roundtrip(&mut app).unwrap();
        draw(&shm, &game, &handle, (800, 600));
        let _mapped = queue.roundtrip(&mut app).unwrap();
        let started = Instant::now();
        while app.focused != Some(game.clone()) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "game never receives keyboard focus"
            );
            let _focused = queue.roundtrip(&mut app).unwrap();
        }
        ready.send(0).unwrap();
        let helper = compositor.create_surface(&handle, ());
        let helper_xdg = wm_base.get_xdg_surface(&helper, &handle, ());
        let helper_top = helper_xdg.get_toplevel(&handle, ());
        for stage in 1..=9 {
            while commands.try_recv().is_err() {
                let _dispatched = queue.roundtrip(&mut app).unwrap();
            }
            advance_helper(stage, &helper_top, &helper, &shm, &handle);
            let _settled = queue.roundtrip(&mut app).unwrap();
            if !matches!(stage, 3 | 6 | 7 | 9) {
                assert_eq!(
                    app.focused,
                    Some(game.clone()),
                    "helper keeps keyboard focus on game at stage {stage}"
                );
            }
            if stage == 1 {
                let (width, height, _) = app.configures.last().unwrap();
                assert_eq!((*width, *height), (0, 0), "helper receives no width preset");
            }
            ready.send(stage).unwrap();
        }
        while commands.try_recv().is_err() {
            let _dispatched = queue.roundtrip(&mut app).unwrap();
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

    #[test]
    fn helper_is_skipped_until_it_gets_a_title_or_grows() {
        use tehuti_core::layout::Command;

        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        attach_output(&display, &mut compositor);
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (ready, stages) = mpsc::channel();
        let (commands, receiver) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, &ready, &receiver));
        let started = Instant::now();
        let mut game = None;
        for stage in 0..=9 {
            loop {
                assert!(started.elapsed() < Duration::from_secs(20), "client timed out");
                pump(&mut display, &mut compositor, Duration::from_millis(1));
                if stages.try_recv() == Ok(stage) {
                    break;
                }
            }
            let snapshot = compositor.layout_snapshot();
            if stage == 0 {
                game = snapshot.focused_window;
            }
            let output = &snapshot.outputs[0];
            let workspace = &output.workspaces[output.active_workspace];
            assert_eq!(
                workspace.columns.len(),
                if matches!(stage, 3 | 6 | 7 | 9) { 2 } else { 1 },
                "stage {stage}"
            );
            if matches!(stage, 3 | 6 | 7 | 9) {
                compositor.apply_layout_command(Command::FocusColumnFirst);
                assert_eq!(compositor.layout_snapshot().focused_window, game);
                compositor.apply_layout_command(Command::FocusColumnRight);
                assert_ne!(compositor.layout_snapshot().focused_window, game);
            } else {
                assert_eq!(snapshot.focused_window, game, "mapping a helper keeps game focus");
                for command in [
                    Command::FocusColumnRight,
                    Command::FocusColumnLeft,
                    Command::FocusWindowDownOrTop,
                ] {
                    compositor.apply_layout_command(command);
                    assert_eq!(compositor.layout_snapshot().focused_window, game);
                }
            }
            commands.send(()).unwrap();
        }
        while !thread.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(20));
            pump(&mut display, &mut compositor, Duration::from_millis(1));
        }
        thread.join().unwrap();
    }
}
