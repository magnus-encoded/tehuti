//! Screen capture is exercised by real Wayland clients.
#[cfg(test)]
mod capture {
    use core::time::Duration;
    use smithay::reexports::wayland_server::{
        Display, ListeningSocket,
        backend::{ClientData, ClientId, DisconnectReason},
    };
    use std::{
        fs::File,
        os::{
            fd::AsFd,
            unix::{fs::FileExt, net::UnixStream},
        },
        sync::Arc,
        thread,
        time::Instant,
    };
    use tehuti::state::{ClientState, Compositor};
    use wayland_client::{
        Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_buffer::WlBuffer,
            wl_output::WlOutput,
            wl_registry::WlRegistry,
            wl_shm::{self, WlShm},
            wl_shm_pool::WlShmPool,
        },
    };
    use wayland_protocols::ext::{
        image_capture_source::v1::client::{
            ext_image_capture_source_v1::ExtImageCaptureSourceV1,
            ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
        },
        image_copy_capture::v1::client::{
            ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
            ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
            ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
        },
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::{self, XdgToplevel},
        xdg_wm_base::{self, XdgWmBase},
    };
    use wayland_protocols_wlr::screencopy::v1::client::{
        zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
        zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    };

    #[derive(Default)]
    struct App {
        size: Option<(u32, u32)>,
        ready: bool,
        failed: bool,
        damage: bool,
        format: Option<wl_shm::Format>,
        transform: Option<wayland_client::protocol::wl_output::Transform>,
        configured: Option<u32>,
        window_size: (i32, i32),
    }
    impl Dispatch<WlRegistry, GlobalListContents> for App {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    delegate_noop!(App: ignore WlOutput);
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);
    delegate_noop!(App: ignore ExtOutputImageCaptureSourceManagerV1);
    delegate_noop!(App: ignore ExtImageCaptureSourceV1);
    delegate_noop!(App: ignore ExtImageCopyCaptureManagerV1);
    delegate_noop!(App: ignore ZwlrScreencopyManagerV1);

    delegate_noop!(App: ignore wayland_client::protocol::wl_compositor::WlCompositor);
    delegate_noop!(App: ignore wayland_client::protocol::wl_surface::WlSurface);
    impl Dispatch<XdgWmBase, ()> for App {
        fn event(
            _: &mut Self,
            base: &XdgWmBase,
            event: xdg_wm_base::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = event {
                base.pong(serial);
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
                surface.ack_configure(serial);
                app.configured = Some(serial);
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
                app.window_size = (width.max(64), height.max(64));
            }
        }
    }

    impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ExtImageCopyCaptureSessionV1,
            event: ext_image_copy_capture_session_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                    app.size = Some((width, height));
                }
                ext_image_copy_capture_session_v1::Event::ShmFormat { format } => {
                    app.format = format.into_result().ok();
                }
                ext_image_copy_capture_session_v1::Event::Stopped => app.failed = true,
                ext_image_copy_capture_session_v1::Event::DmabufDevice { .. }
                | ext_image_copy_capture_session_v1::Event::DmabufFormat { .. }
                | ext_image_copy_capture_session_v1::Event::Done
                | _ => {}
            }
        }
    }
    impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ExtImageCopyCaptureFrameV1,
            event: ext_image_copy_capture_frame_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_image_copy_capture_frame_v1::Event::Ready => app.ready = true,
                ext_image_copy_capture_frame_v1::Event::Failed { .. } => app.failed = true,
                ext_image_copy_capture_frame_v1::Event::Damage { .. } => app.damage = true,
                ext_image_copy_capture_frame_v1::Event::Transform { transform } => {
                    app.transform = transform.into_result().ok();
                }
                ext_image_copy_capture_frame_v1::Event::PresentationTime { .. } | _ => {}
            }
        }
    }
    impl Dispatch<ZwlrScreencopyFrameV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ZwlrScreencopyFrameV1,
            event: zwlr_screencopy_frame_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                zwlr_screencopy_frame_v1::Event::Buffer {
                    width,
                    height,
                    format,
                    ..
                } => {
                    app.size = Some((width, height));
                    app.format = format.into_result().ok();
                }
                zwlr_screencopy_frame_v1::Event::Ready { .. } => app.ready = true,
                zwlr_screencopy_frame_v1::Event::Failed => app.failed = true,
                zwlr_screencopy_frame_v1::Event::Damage { .. } => app.damage = true,
                zwlr_screencopy_frame_v1::Event::Flags { .. }
                | zwlr_screencopy_frame_v1::Event::LinuxDmabuf { .. }
                | zwlr_screencopy_frame_v1::Event::BufferDone
                | _ => {}
            }
        }
    }

    fn buffer(
        shm: &WlShm,
        handle: &QueueHandle<App>,
        width: u32,
        height: u32,
        padding: u32,
    ) -> (File, WlBuffer) {
        let stride = width.checked_mul(4).unwrap().checked_add(padding).unwrap();
        let length = stride.checked_mul(height).unwrap().checked_add(32).unwrap();
        let file = tempfile::tempfile().unwrap();
        file.set_len(u64::from(length)).unwrap();
        file.write_all_at(&vec![0x55; usize::try_from(length).unwrap()], 0)
            .unwrap();
        let pool = shm.create_pool(file.as_fd(), i32::try_from(length).unwrap(), handle, ());
        let buffer = pool.create_buffer(
            16,
            i32::try_from(width).unwrap(),
            i32::try_from(height).unwrap(),
            i32::try_from(stride).unwrap(),
            wl_shm::Format::Abgr8888,
            handle,
            (),
        );
        pool.destroy();
        (file, buffer)
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Ext,
        Nested,
        Window,
        Wlr(u32),
        Region,
        WrongSize,
        Truncated,
        Duplicate,
        MissingBuffer,
        InvalidDamage,
    }

    struct Capture {
        session: Option<ExtImageCopyCaptureSessionV1>,
        frame: Option<ExtImageCopyCaptureFrameV1>,
        wlr: Option<ZwlrScreencopyFrameV1>,
    }

    fn start_capture(
        globals: &wayland_client::globals::GlobalList,
        queue: &mut wayland_client::EventQueue<App>,
        app: &mut App,
        mode: Mode,
    ) -> Capture {
        let handle = queue.handle();
        let output: WlOutput = globals.bind(&handle, 1..=4, ()).unwrap();
        let ext = !matches!(mode, Mode::Wlr(_) | Mode::Region);
        if ext {
            let source_manager: ExtOutputImageCaptureSourceManagerV1 =
                globals.bind(&handle, 1..=1, ()).unwrap();
            let manager: ExtImageCopyCaptureManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
            let source = source_manager.create_source(&output, &handle, ());
            let session = manager.create_session(
                &source,
                ext_image_copy_capture_manager_v1::Options::empty(),
                &handle,
                (),
            );
            let _constraints = queue.roundtrip(app).unwrap();
            source.destroy();
            source_manager.destroy();
            manager.destroy();
            let frame = session.create_frame(&handle, ());
            Capture {
                session: Some(session),
                frame: Some(frame),
                wlr: None,
            }
        } else {
            let version = if let Mode::Wlr(version) = mode { version } else { 3 };
            let manager: ZwlrScreencopyManagerV1 = globals.bind(&handle, version..=version, ()).unwrap();
            let frame = if matches!(mode, Mode::Region) {
                manager.capture_output_region(0, &output, 0, 0, 32, 24, &handle, ())
            } else {
                manager.capture_output(0, &output, &handle, ())
            };
            manager.destroy();
            let _constraints = queue.roundtrip(app).unwrap();
            Capture {
                session: None,
                frame: None,
                wlr: Some(frame),
            }
        }
    }

    fn request_capture(capture: &Capture, target: &WlBuffer, mode: Mode, width: u32, height: u32) {
        if let Some(frame) = &capture.frame {
            if !matches!(mode, Mode::MissingBuffer) {
                frame.attach_buffer(target);
            }
            frame.damage_buffer(
                if matches!(mode, Mode::InvalidDamage) {
                    -1
                } else {
                    0
                },
                0,
                i32::try_from(width).unwrap(),
                i32::try_from(height).unwrap(),
            );
            frame.capture();
            if matches!(mode, Mode::Duplicate) {
                frame.capture();
            }
        }
        if let Some(legacy) = &capture.wlr {
            if legacy.version() >= 2 {
                legacy.copy_with_damage(target);
            } else {
                legacy.copy(target);
            }
        }
    }

    fn assert_pixels(file: &File, width: u32, padding: u32, ext: bool, y: u32) {
        let stride = width.checked_mul(4).unwrap().checked_add(padding).unwrap();
        let mut pixel = [0; 4];
        file.read_exact_at(&mut pixel, 0).unwrap();
        assert_eq!(
            pixel, [0x55; 4],
            "capture preserves bytes before the buffer offset"
        );
        if ext {
            file.read_exact_at(
                &mut pixel,
                u64::from(width.checked_mul(4).unwrap().checked_add(16).unwrap()),
            )
            .unwrap();
            assert_eq!(pixel, [0x55; 4], "capture preserves row padding");
        }
        file.read_exact_at(
            &mut pixel,
            u64::from(stride.checked_mul(y).unwrap().checked_add(80).unwrap()),
        )
        .unwrap();
        assert_eq!(pixel, [32, 32, 32, 255], "capture includes the visible wallpaper");
    }

    fn finish_capture(
        capture: Capture,
        target: &WlBuffer,
        queue: &mut wayland_client::EventQueue<App>,
        app: &mut App,
    ) {
        if let Some(frame) = capture.frame {
            frame.destroy();
        }
        if let Some(legacy) = capture.wlr {
            legacy.destroy();
        }
        let _destroyed = queue.roundtrip(app).unwrap();
        if let Some(session) = capture.session {
            app.ready = false;
            let frame = session.create_frame(&queue.handle(), ());
            frame.attach_buffer(target);
            frame.capture();
            let _second = queue.roundtrip(app).unwrap();
            assert!(
                app.ready,
                "destroying a frame permits another capture in the session"
            );
            frame.destroy();
            session.destroy();
        }
    }

    fn paint_window(
        globals: &wayland_client::globals::GlobalList,
        queue: &mut wayland_client::EventQueue<App>,
        app: &mut App,
    ) -> (
        wayland_client::protocol::wl_surface::WlSurface,
        XdgSurface,
        XdgToplevel,
        File,
    ) {
        let handle = queue.handle();
        let compositor: wayland_client::protocol::wl_compositor::WlCompositor =
            globals.bind(&handle, 1..=4, ()).unwrap();
        let manager: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let xdg = manager.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg.get_toplevel(&handle, ());
        toplevel.set_app_id("capture-test".into());
        surface.commit();
        while app.configured.is_none() {
            let _configured = queue.roundtrip(app).unwrap();
        }
        let (width, height) = app.window_size;
        let (file, painted) = buffer(
            &shm,
            &handle,
            u32::try_from(width).unwrap(),
            u32::try_from(height).unwrap(),
            0,
        );
        let count = usize::try_from(width)
            .unwrap()
            .checked_mul(usize::try_from(height).unwrap())
            .unwrap();
        file.write_all_at(&[17, 43, 197, 255].repeat(count), 16).unwrap();
        surface.attach(Some(&painted), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        let _mapped = queue.roundtrip(app).unwrap();
        (surface, xdg, toplevel, file)
    }

    fn assert_captured_pixels(file: &File, width: u32, height: u32, padding: u32, app: &App, mode: Mode) {
        if matches!(mode, Mode::Window) {
            let mut pixels = vec![0; usize::try_from(file.metadata().unwrap().len()).unwrap()];
            file.read_exact_at(&mut pixels, 0).unwrap();
            assert!(
                pixels
                    .chunks_exact(4)
                    .filter(|pixel| *pixel == [17, 43, 197, 255])
                    .count()
                    > 1000,
                "capture includes mapped client window pixels"
            );
            return;
        }
        let y = if matches!(mode, Mode::Nested) {
            assert_eq!(
                app.transform,
                Some(wayland_client::protocol::wl_output::Transform::Flipped180)
            );
            height.checked_sub(17).unwrap()
        } else {
            16
        };
        assert_pixels(
            file,
            width,
            padding,
            !matches!(mode, Mode::Wlr(_) | Mode::Region),
            y,
        );
    }

    fn client(stream: UnixStream, mode: Mode) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let mut app = App::default();
        let _window = matches!(mode, Mode::Window).then(|| paint_window(&globals, &mut queue, &mut app));
        let capture = start_capture(&globals, &mut queue, &mut app, mode);
        let ext = capture.frame.is_some();
        let (width, height) = app.size.unwrap();
        assert_eq!(app.format, Some(wl_shm::Format::Abgr8888));
        let padding = if ext { 12 } else { 0 };
        let actual_width = if matches!(mode, Mode::WrongSize) {
            width.checked_sub(1).unwrap()
        } else {
            width
        };
        let (file, target) = buffer(&shm, &handle, actual_width, height, padding);
        if matches!(mode, Mode::Truncated) {
            file.set_len(0).unwrap();
        }
        request_capture(&capture, &target, mode, width, height);
        if matches!(mode, Mode::Duplicate | Mode::MissingBuffer | Mode::InvalidDamage) {
            assert!(
                queue.roundtrip(&mut app).is_err(),
                "invalid requests must disconnect only their client"
            );
            return;
        }
        let _captured = queue.roundtrip(&mut app).unwrap();
        if matches!(mode, Mode::WrongSize | Mode::Truncated) {
            assert!(app.failed && !app.ready, "invalid buffers cannot become ready");
            return;
        }
        assert!(app.ready && !app.failed, "capture must finish successfully");
        if ext || capture.wlr.as_ref().is_some_and(|legacy| legacy.version() >= 2) {
            assert!(app.damage, "capture reports damage");
        }
        assert_captured_pixels(&file, width, height, padding, &app, mode);
        finish_capture(capture, &target, &mut queue, &mut app);
    }

    fn pump(display: &mut Display<Compositor>, compositor: &mut Compositor) {
        let _dispatched = display.dispatch_clients(compositor).unwrap();
        compositor.space.refresh();
        display.flush_clients().unwrap();
        thread::sleep(Duration::from_millis(1));
    }
    fn run(mode: Mode) {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let task = thread::spawn(move || client(stream, mode));
        let start = Instant::now();
        while !task.is_finished() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "capture client timed out"
            );
            pump(&mut display, &mut compositor);
        }
        task.join().unwrap();
    }

    #[test]
    fn ext_captures_pixels_with_offsets_and_padding_and_reuses_sessions() {
        run(Mode::Ext);
    }
    #[test]
    fn capture_includes_mapped_client_pixels() {
        run(Mode::Window);
    }
    #[test]
    fn wlr_captures_outputs_for_every_protocol_version() {
        for version in 1..=3 {
            run(Mode::Wlr(version));
        }
    }
    #[test]
    fn wlr_captures_an_output_region() {
        run(Mode::Region);
    }
    #[test]
    fn invalid_client_buffers_fail_without_stopping_the_compositor() {
        run(Mode::WrongSize);
        run(Mode::Truncated);
        run(Mode::Ext);
    }
    #[test]
    fn invalid_frame_requests_disconnect_the_client() {
        for mode in [Mode::Duplicate, Mode::MissingBuffer, Mode::InvalidDamage] {
            run(mode);
        }
    }

    fn session_client(stream: UnixStream, ready: &std::sync::mpsc::Sender<()>, removed: bool) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let output: WlOutput = globals.bind(&handle, 1..=4, ()).unwrap();
        let source_manager: ExtOutputImageCaptureSourceManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
        let manager: ExtImageCopyCaptureManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
        let source = source_manager.create_source(&output, &handle, ());
        let session = manager.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &handle,
            (),
        );
        let mut app = App::default();
        let _constraints = queue.roundtrip(&mut app).unwrap();
        ready.send(()).unwrap();
        while if removed {
            !app.failed
        } else {
            app.size != Some((640, 480))
        } {
            let _events = queue.blocking_dispatch(&mut app).unwrap();
        }
        if removed {
            let frame = session.create_frame(&handle, ());
            let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
            let (_file, target) = buffer(&shm, &handle, 640, 480, 0);
            app.failed = false;
            frame.attach_buffer(&target);
            frame.capture();
            let _capture = queue.roundtrip(&mut app).unwrap();
            assert!(app.failed && !app.ready, "a removed output cannot be captured");
        }
    }

    fn changed_output(removed: bool) {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let output = compositor.output.clone().unwrap();
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (ready, received) = std::sync::mpsc::channel();
        let task = thread::spawn(move || session_client(stream, &ready, removed));
        let start = Instant::now();
        while !task.is_finished() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "session change timed out"
            );
            pump(&mut display, &mut compositor);
            if received.try_recv().is_ok() {
                if removed {
                    compositor.space.unmap_output(&output);
                } else {
                    output.change_current_state(
                        Some(smithay::output::Mode {
                            size: (640, 480).into(),
                            refresh: 60_000,
                        }),
                        None,
                        None,
                        None,
                    );
                }
                compositor.advance_time();
            }
        }
        task.join().unwrap();
    }

    #[test]
    fn capture_constraints_follow_output_resize() {
        changed_output(false);
    }
    #[test]
    fn removing_an_output_stops_its_capture_sessions() {
        changed_output(true);
    }

    #[derive(Debug)]
    struct Untrusted;
    impl ClientData for Untrusted {
        fn initialized(&self, _: ClientId) {}
        fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
    }
    #[test]
    fn clients_without_tehuti_session_trust_cannot_discover_capture_globals() {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(Untrusted))
            .unwrap();
        let task = thread::spawn(move || {
            let connection = Connection::from_socket(stream).unwrap();
            let (globals, _) = registry_queue_init::<App>(&connection).unwrap();
            assert!(
                globals.contents().with_list(|list| list
                    .iter()
                    .all(|global| !global.interface.contains("capture")
                        && !global.interface.contains("screencopy"))),
                "untrusted clients must not see capture globals"
            );
        });
        let start = Instant::now();
        while !task.is_finished() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "registry client timed out"
            );
            pump(&mut display, &mut compositor);
        }
        task.join().unwrap();
    }

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _killed = self.0.kill();
            let _waited = self.0.wait();
        }
    }

    fn x_display() -> Option<(ChildGuard, String)> {
        use std::io::BufRead;
        let process = std::process::Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "1280x720x24",
                "-nolisten",
                "tcp",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let mut process = ChildGuard(process);
        let output = process.0.stdout.take()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let _reader = thread::spawn(move || {
            let mut number = String::new();
            let _line = std::io::BufReader::new(output).read_line(&mut number).unwrap();
            sender.send(number).unwrap();
        });
        let number = receiver.recv_timeout(Duration::from_secs(10)).ok()?;
        Some((process, format!(":{}", number.trim())))
    }

    fn nested_socket(directory: &std::path::Path, process: &mut ChildGuard) -> std::path::PathBuf {
        let start = Instant::now();
        loop {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "nested startup timed out"
            );
            assert!(
                process.0.try_wait().unwrap().is_none(),
                "nested compositor exited"
            );
            if let Some(path) = std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("wayland-"))
                        && path.extension().is_none()
                })
            {
                return path;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn nested_compositor(
        directory: &std::path::Path,
        contexts: &std::path::Path,
        display: &str,
    ) -> ChildGuard {
        ChildGuard(
            std::process::Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .arg(contexts)
                .env("DISPLAY", display)
                .env("WINIT_UNIX_BACKEND", "x11")
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("WAYLAND_SOCKET")
                .env("XDG_RUNTIME_DIR", directory)
                .env("XDG_CONFIG_HOME", directory.join("config"))
                .env("XDG_DATA_HOME", directory.join("data"))
                .env("XDG_CACHE_HOME", directory.join("cache"))
                .env("TEHUTI_SCALE", "1")
                .env("TEHUTI_TEST_SIZE", "1280x720")
                .env("LIBGL_ALWAYS_SOFTWARE", "1")
                .spawn()
                .unwrap(),
        )
    }

    fn nested_grim(socket: &std::path::Path, path: &std::path::Path, directory: &std::path::Path) {
        let mut grim = ChildGuard(
            std::process::Command::new("grim")
                .arg(path)
                .env("WAYLAND_DISPLAY", socket)
                .env("XDG_RUNTIME_DIR", directory)
                .env_remove("WAYLAND_SOCKET")
                .spawn()
                .unwrap(),
        );
        let start = Instant::now();
        loop {
            assert!(start.elapsed() < Duration::from_secs(10), "nested grim timed out");
            if let Some(status) = grim.0.try_wait().unwrap() {
                assert!(status.success(), "grim must capture the nested renderer");
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn ext_and_grim_capture_the_gles_nested_session() {
        let Some((_host, display)) = x_display() else {
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let contexts = directory.path().join("contexts");
        std::fs::create_dir(&contexts).unwrap();
        std::fs::write(contexts.join("context.toml"), "app_id = '*'\nbuttons = {}\n").unwrap();
        let mut compositor = nested_compositor(directory.path(), &contexts, &display);
        let socket = nested_socket(directory.path(), &mut compositor);
        let stream = UnixStream::connect(&socket).unwrap();
        let task = thread::spawn(move || client(stream, Mode::Nested));
        let start = Instant::now();
        while !task.is_finished() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "nested capture timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
        task.join().unwrap();
        let path = directory.path().join("nested.png");
        nested_grim(&socket, &path, directory.path());
        let image = image::open(path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (1280, 720));
        assert_eq!(*image.get_pixel(16, 16), image::Rgba([32, 32, 32, 255]));
    }

    #[test]
    fn grim_captures_a_nested_tehuti_output() {
        if std::process::Command::new("grim").arg("-h").output().is_err() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("wayland-capture");
        let socket = ListeningSocket::bind_absolute(socket_path.clone()).unwrap();
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let path = directory.path().join("grim.png");
        let mut grim = std::process::Command::new("grim")
            .arg(&path)
            .env("WAYLAND_DISPLAY", &socket_path)
            .env_remove("WAYLAND_SOCKET")
            .spawn()
            .unwrap();
        let start = Instant::now();
        loop {
            assert!(start.elapsed() < Duration::from_secs(10), "grim timed out");
            if let Some(stream) = socket.accept().unwrap() {
                let _client = display
                    .handle()
                    .insert_client(stream, Arc::new(ClientState::default()))
                    .unwrap();
            }
            pump(&mut display, &mut compositor);
            if let Some(status) = grim.try_wait().unwrap() {
                assert!(status.success(), "grim must capture successfully");
                break;
            }
        }
        let image = image::open(path).unwrap().to_rgba8();
        assert_eq!(*image.get_pixel(16, 16), image::Rgba([32, 32, 32, 255]));
        assert_eq!(
            *image.get_pixel(image.width() / 2, image.height() / 2),
            image::Rgba([0, 0, 0, 255])
        );
    }
}
