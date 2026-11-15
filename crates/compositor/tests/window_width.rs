//! `[window] width` in a context: windows open at that fraction of the output, and pixel or
//! decimal values are rewritten to a fraction in the file without ever tearing it.

#[cfg(test)]
mod window_width {
    use core::time::Duration;
    use std::{
        os::{fd::AsFd, unix::net::UnixStream},
        path::Path,
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
        width_rewrite,
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

    const APP_ID: &str = "org.example.Game";
    const SCREEN: i32 = 3840;

    #[derive(Default)]
    struct App;

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
            _: &mut Self,
            _: &XdgToplevel,
            _: xdg_toplevel::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlOutput);
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);

    fn client(stream: UnixStream, ready: &mpsc::Sender<()>, release: &mpsc::Receiver<()>) {
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
        toplevel.set_app_id(APP_ID.into());
        toplevel.set_title("window".into());
        surface.commit();
        let mut app = App;
        let _first = queue.roundtrip(&mut app).unwrap();
        let _second = queue.roundtrip(&mut app).unwrap();
        surface.commit();
        let _unmapped = queue.roundtrip(&mut app).unwrap();
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
    }

    fn add_output(compositor: &mut Compositor, display: &Display<Compositor>) {
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
                size: (SCREEN, 2160).into(),
                refresh: 60_000,
            },
            1,
        );
        compositor.space.map_output(&output, (0, 0));
        compositor.output = Some(output);
        compositor.fill_output();
    }

    /// Opens one window of `APP_ID` against a context file holding `source`, and returns the
    /// compositor with the opened window and the file's path.
    fn open_window(source: &str, directory: &Path) -> Compositor {
        let path = directory.join("context.toml");
        std::fs::write(&path, source).unwrap();
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let context = tehuti_protocol::parse_context(source).unwrap();
        compositor.set_context_files(vec![(path, context)]);
        add_output(&mut compositor, &display);
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (ready, mapped) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, &ready, &wait));
        let started = Instant::now();
        while mapped.try_recv().is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            compositor.space.refresh();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        release.send(()).unwrap();
        thread.join().unwrap();
        compositor
    }

    fn context(width: &str) -> String {
        format!("app_id = '{APP_ID}'\nbuttons = {{}}\n[window]\nwidth = {width}\n")
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "compare a tile width with a share of the output"
    )]
    fn opened_share(compositor: &Compositor) -> (Option<tehuti_core::layout::PresetSize>, f64) {
        let snapshot = compositor.layout_snapshot();
        let output = &snapshot.outputs[0];
        let column = &output.workspaces[output.active_workspace].columns[0];
        (
            column.width,
            column.tiles[0].tile.width / output.geometry.area.width,
        )
    }

    #[test]
    fn a_fraction_opens_at_that_share_of_the_output_width() {
        let directory = tempfile::tempdir().unwrap();
        let source = context("\"2/3\"");
        let compositor = open_window(&source, directory.path());
        let (width, share) = opened_share(&compositor);
        assert_eq!(
            width,
            Some(tehuti_core::layout::PresetSize::Proportion(2.0 / 3.0))
        );
        assert!((share - 2.0 / 3.0).abs() < 0.05, "{share}");
        assert_eq!(
            std::fs::read_to_string(directory.path().join("context.toml")).unwrap(),
            source
        );
    }

    #[test]
    fn a_pixel_width_opens_as_a_fraction_and_is_rewritten_in_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let compositor = open_window(&context("2560"), directory.path());
        let (width, _) = opened_share(&compositor);
        assert_eq!(
            width,
            Some(tehuti_core::layout::PresetSize::Proportion(2.0 / 3.0))
        );
        let written = std::fs::read_to_string(directory.path().join("context.toml")).unwrap();
        assert!(
            written.contains("width = \"2/3\"  # was 2560 px on 3840 px\n"),
            "{written}"
        );
        assert!(written.starts_with(&format!("app_id = '{APP_ID}'\nbuttons = {{}}\n[window]\n")));
    }

    #[test]
    fn a_pixel_width_wider_than_the_screen_is_an_error_not_a_window_size() {
        let directory = tempfile::tempdir().unwrap();
        let source = context("5000");
        let compositor = open_window(&source, directory.path());
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("WINDOW WIDTH IS WIDER THAN THE SCREEN")
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("context.toml")).unwrap(),
            source
        );
    }

    #[test]
    fn a_decimal_is_rewritten_through_the_file_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("context.toml");
        let source = "# keep\napp_id = 'x'\nbuttons = {}\n[window]\nwidth = 0.667\nheight = 0.5\n";
        std::fs::write(&path, source).unwrap();
        assert!(width_rewrite::rewrite_file(&path, (2, 3), None).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# keep\napp_id = 'x'\nbuttons = {}\n[window]\nwidth = \"2/3\"  # was 0.667\nheight = 0.5\n"
        );
        let names: Vec<_> = std::fs::read_dir(directory.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "no temporary file is left behind");
    }

    #[test]
    fn a_file_without_a_width_line_is_left_alone() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("context.toml");
        std::fs::write(&path, "[window]\nheight = 0.5\n").unwrap();
        assert!(!width_rewrite::rewrite_file(&path, (2, 3), None).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[window]\nheight = 0.5\n"
        );
    }

    #[test]
    fn a_directory_that_cannot_be_written_leaves_the_file_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("context.toml");
        let source = "[window]\nwidth = 1280\n";
        std::fs::write(&path, source).unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = width_rewrite::rewrite_file(&path, (2, 3), Some(1920));
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root ignores directory permissions; there the rewrite legitimately succeeds.
        if result.is_err() {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn a_failed_replacement_removes_its_temporary_file_and_keeps_the_original() {
        let directory = tempfile::tempdir().unwrap();
        // A directory in place of the file: the rename cannot succeed.
        let path = directory.path().join("context.toml");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("inner"), "x").unwrap();
        assert!(width_rewrite::replace_file(&path, "new").is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(path.join("inner").exists());
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        assert!(width_rewrite::rewrite_file(&directory.path().join("absent.toml"), (1, 2), None).is_err());
    }
}
