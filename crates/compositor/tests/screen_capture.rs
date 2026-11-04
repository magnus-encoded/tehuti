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

    impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ExtImageCopyCaptureSessionV1,
            event: ext_image_copy_capture_session_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                    app.size = Some((width, height))
                }
                ext_image_copy_capture_session_v1::Event::ShmFormat { format } => {
                    app.format = format.into_result().ok()
                }
                ext_image_copy_capture_session_v1::Event::Stopped => app.failed = true,
                _ => {}
            }
        }
    }
    impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ExtImageCopyCaptureFrameV1,
            event: ext_image_copy_capture_frame_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_image_copy_capture_frame_v1::Event::Ready => app.ready = true,
                ext_image_copy_capture_frame_v1::Event::Failed { .. } => app.failed = true,
                ext_image_copy_capture_frame_v1::Event::Damage { .. } => app.damage = true,
                _ => {}
            }
        }
    }
    impl Dispatch<ZwlrScreencopyFrameV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ZwlrScreencopyFrameV1,
            event: zwlr_screencopy_frame_v1::Event,
            _: &(),
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
                _ => {}
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
        Wlr(u32),
        Region,
        WrongSize,
        Truncated,
        Duplicate,
        MissingBuffer,
        InvalidDamage,
    }

    fn client(stream: UnixStream, mode: Mode) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let output: WlOutput = globals.bind(&handle, 1..=4, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let mut app = App::default();
        let ext = !matches!(mode, Mode::Wlr(_) | Mode::Region);
        let (session, frame, wlr) = if ext {
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
            let _constraints = queue.roundtrip(&mut app).unwrap();
            source.destroy();
            source_manager.destroy();
            manager.destroy();
            let frame = session.create_frame(&handle, ());
            (Some(session), Some(frame), None)
        } else {
            let version = if let Mode::Wlr(version) = mode { version } else { 3 };
            let manager: ZwlrScreencopyManagerV1 = globals.bind(&handle, version..=version, ()).unwrap();
            let frame = if matches!(mode, Mode::Region) {
                manager.capture_output_region(0, &output, 0, 0, 32, 24, &handle, ())
            } else {
                manager.capture_output(0, &output, &handle, ())
            };
            manager.destroy();
            let _constraints = queue.roundtrip(&mut app).unwrap();
            (None, None, Some(frame))
        };
        let (width, height) = app.size.unwrap();
        assert_eq!(app.format, Some(wl_shm::Format::Abgr8888));
        let padding = if ext { 12 } else { 0 };
        let actual_width = if matches!(mode, Mode::WrongSize) {
            width - 1
        } else {
            width
        };
        let (file, target) = buffer(&shm, &handle, actual_width, height, padding);
        if matches!(mode, Mode::Truncated) {
            file.set_len(0).unwrap();
        }
        if let Some(frame) = &frame {
            if !matches!(mode, Mode::MissingBuffer) {
                frame.attach_buffer(&target);
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
        if let Some(frame) = &wlr {
            if frame.version() >= 2 {
                frame.copy_with_damage(&target);
            } else {
                frame.copy(&target);
            }
        }
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
        if ext || wlr.as_ref().is_some_and(|frame| frame.version() >= 2) {
            assert!(app.damage, "capture reports damage");
        }
        let stride = width.checked_mul(4).unwrap().checked_add(padding).unwrap();
        let mut pixel = [0; 4];
        file.read_exact_at(&mut pixel, 0).unwrap();
        assert_eq!(
            pixel, [0x55; 4],
            "capture preserves bytes before the buffer offset"
        );
        if ext {
            file.read_exact_at(&mut pixel, u64::from(16 + width * 4)).unwrap();
            assert_eq!(pixel, [0x55; 4], "capture preserves row padding");
        }
        file.read_exact_at(&mut pixel, u64::from(16 + 16 * stride + 16 * 4))
            .unwrap();
        assert_eq!(pixel, [32, 32, 32, 255], "capture includes the visible wallpaper");
        if let Some(frame) = frame {
            frame.destroy();
        }
        if let Some(frame) = wlr {
            frame.destroy();
        }
        let _destroyed = queue.roundtrip(&mut app).unwrap();
        if let Some(session) = session {
            app.ready = false;
            let frame = session.create_frame(&handle, ());
            frame.attach_buffer(&target);
            frame.capture();
            let _second = queue.roundtrip(&mut app).unwrap();
            assert!(
                app.ready,
                "destroying a frame permits another capture in the session"
            );
            frame.destroy();
            session.destroy();
        }
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
