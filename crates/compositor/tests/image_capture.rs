//! Clients capture a whole output through ext-image-copy-capture-v1 into shm buffers.

#[cfg(test)]
mod image_capture {
    use core::{cell::RefCell, time::Duration};
    use std::{
        io::{Read, Seek, SeekFrom, Write},
        os::{fd::AsFd, unix::net::UnixStream},
        rc::Rc,
        sync::{Arc, mpsc},
        thread,
        time::Instant,
    };

    use proptest::prelude::*;
    use smithay::{
        output::Mode,
        reexports::{
            wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::FailureReason,
            wayland_server::{Display, protocol::wl_shm::Format as ServerFormat},
        },
    };
    use tehuti::{
        image_capture::{BufferMismatch, FORMATS, encode},
        scale,
        state::{ClientState, Compositor},
    };
    use tehuti_core::secret::{SecretProvider, SecretProviderUnavailable, SecretValue};
    use tehuti_protocol::SecretReference;
    use wayland_client::{
        Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
        globals::{GlobalList, GlobalListContents, registry_queue_init},
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
    use wayland_protocols::ext::{
        image_capture_source::v1::client::{
            ext_image_capture_source_v1::ExtImageCaptureSourceV1,
            ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
        },
        image_copy_capture::v1::client::{
            ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
            ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
            ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
        },
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::XdgToplevel,
        xdg_wm_base::{self, XdgWmBase},
    };

    fn pixel(format: ServerFormat, bytes: [u8; 4]) -> [u8; 4] {
        let [first, second, third, fourth] = bytes;
        if matches!(format, ServerFormat::Argb8888 | ServerFormat::Xrgb8888) {
            [third, second, first, fourth]
        } else {
            bytes
        }
    }

    fn generated_image() -> impl Strategy<Value = image::RgbaImage> {
        (1_u32..12, 1_u32..12).prop_flat_map(|(width, height)| {
            proptest::collection::vec(
                any::<u8>(),
                usize::try_from(width.saturating_mul(height).saturating_mul(4)).unwrap_or(0),
            )
            .prop_map(move |bytes| image::RgbaImage::from_raw(width, height, bytes).unwrap_or_default())
        })
    }

    proptest! {
        #[test]
        fn encoded_buffers_decode_to_the_frame_and_keep_row_padding(
            image in generated_image(),
            format in proptest::sample::select(FORMATS.to_vec()),
            padding in 0_usize..9,
            filler in any::<u8>(),
        ) {
            let (width, height) = image.dimensions();
            let row = usize::try_from(width).unwrap_or(0).saturating_mul(4);
            let stride = row.saturating_add(padding);
            let mut buffer = vec![filler; stride.saturating_mul(usize::try_from(height).unwrap_or(0))];
            prop_assert_eq!(encode(&image, format, (width, height), stride, &mut buffer), Ok(()));
            for (y, line) in buffer.chunks(stride).enumerate() {
                let (pixels, pad) = line.split_at(row);
                prop_assert!(pad.iter().all(|byte| *byte == filler), "row padding was overwritten");
                for (x, bytes) in pixels.as_chunks::<4>().0.iter().enumerate() {
                    let expected = image.get_pixel(u32::try_from(x).unwrap_or(0), u32::try_from(y).unwrap_or(0)).0;
                    prop_assert_eq!(pixel(format, *bytes), expected);
                }
            }
        }

        #[test]
        fn mismatched_buffers_are_refused(image in generated_image(), short in 1_usize..4) {
            let (width, height) = image.dimensions();
            let row = usize::try_from(width).unwrap_or(0).saturating_mul(4);
            let mut buffer = vec![0; row.saturating_mul(usize::try_from(height).unwrap_or(0))];
            prop_assert_eq!(
                encode(&image, ServerFormat::Argb8888, (width.saturating_add(1), height), row, &mut buffer),
                Err(BufferMismatch::Size)
            );
            prop_assert_eq!(
                encode(&image, ServerFormat::Rgb565, (width, height), row, &mut buffer),
                Err(BufferMismatch::Format)
            );
            prop_assert_eq!(
                encode(&image, ServerFormat::Argb8888, (width, height), row.saturating_sub(1), &mut buffer),
                Err(BufferMismatch::Stride)
            );
            let cut = buffer.len().saturating_sub(short);
            prop_assert_eq!(
                encode(&image, ServerFormat::Argb8888, (width, height), row, &mut buffer[..cut]),
                Err(BufferMismatch::Length)
            );
        }
    }

    #[derive(Debug, Default)]
    struct App {
        size: Option<(u32, u32)>,
        formats: Vec<wl_shm::Format>,
        done: bool,
        stopped: bool,
        outcome: Option<Result<(), u32>>,
        configured: bool,
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
    impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ExtImageCopyCaptureSessionV1,
            event: ext_image_copy_capture_session_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            use ext_image_copy_capture_session_v1::Event;
            if let Event::BufferSize { width, height } = event {
                app.size = Some((width, height));
            } else if let Event::ShmFormat {
                format: wayland_client::WEnum::Value(format),
            } = event
            {
                app.formats.push(format);
            } else if matches!(event, Event::Done) {
                app.done = true;
            } else if matches!(event, Event::Stopped) {
                app.stopped = true;
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
            use ext_image_copy_capture_frame_v1::Event;
            if matches!(event, Event::Ready) {
                app.outcome = Some(Ok(()));
            } else if let Event::Failed { reason } = event {
                app.outcome = Some(Err(reason.into()));
            }
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
                surface.ack_configure(serial);
                app.configured = true;
            }
        }
    }
    delegate_noop!(App: ignore WlOutput);
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore XdgToplevel);
    delegate_noop!(App: ExtOutputImageCaptureSourceManagerV1);
    delegate_noop!(App: ExtImageCaptureSourceV1);
    delegate_noop!(App: ExtImageCopyCaptureManagerV1);

    /// What the client does between announcing the session and capturing.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Plan {
        /// Capture into a buffer this many pixels wider and taller than announced.
        Capture { grow: (u32, u32) },
        /// Map a window, let the compositor raise a secret confirmation, then capture.
        CaptureDuringSecret,
        /// Let the compositor change the output mode, wait for `stopped`, then capture.
        AwaitStop,
    }

    /// What a capturing client saw: buffer size, formats, outcome, and the first pixel of its buffer.
    #[derive(Debug)]
    struct Report {
        size: Option<(u32, u32)>,
        formats: Vec<wl_shm::Format>,
        stopped: bool,
        ready: bool,
        failed: Option<u32>,
        first_pixel: [u8; 4],
    }

    fn shm_file(bytes: i32) -> std::fs::File {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0; usize::try_from(bytes).unwrap()]).unwrap();
        file
    }

    fn map_window(globals: &GlobalList, queue: &mut EventQueue<App>, app: &mut App, shm: &WlShm) {
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 2..=6, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg.get_toplevel(&handle, ());
        toplevel.set_title("Example form".into());
        surface.commit();
        while !app.configured {
            let _dispatched = queue.roundtrip(app).unwrap();
        }
        let file = shm_file(64 * 64 * 4);
        let pool = shm.create_pool(file.as_fd(), 64 * 64 * 4, &handle, ());
        let buffer = pool.create_buffer(0, 64, 64, 64 * 4, wl_shm::Format::Argb8888, &handle, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 64, 64);
        surface.commit();
        for _ in 0..5 {
            let _dispatched = queue.roundtrip(app).unwrap();
        }
    }

    fn capture(
        queue: &mut EventQueue<App>,
        app: &mut App,
        shm: &WlShm,
        session: &ExtImageCopyCaptureSessionV1,
        grow: (u32, u32),
    ) -> [u8; 4] {
        let handle = queue.handle();
        let (width, height) = app.size.unwrap();
        let (width, height) = (width.saturating_add(grow.0), height.saturating_add(grow.1));
        let stride = i32::try_from(width.saturating_mul(4)).unwrap();
        let bytes = stride.saturating_mul(i32::try_from(height).unwrap());
        let mut file = shm_file(bytes);
        let pool = shm.create_pool(file.as_fd(), bytes, &handle, ());
        let (width, height) = (i32::try_from(width).unwrap(), i32::try_from(height).unwrap());
        let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &handle, ());
        let frame = session.create_frame(&handle, ());
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, width, height);
        frame.capture();
        while app.outcome.is_none() {
            let _dispatched = queue.roundtrip(app).unwrap();
        }
        let mut first_pixel = [0; 4];
        let _start = file.seek(SeekFrom::Start(0)).unwrap();
        file.read_exact(&mut first_pixel).unwrap();
        first_pixel
    }

    fn client(
        stream: UnixStream,
        plan: Plan,
        signal: &mpsc::Sender<()>,
        resume: &mpsc::Receiver<()>,
    ) -> Report {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let output: WlOutput = globals.bind(&handle, 1..=4, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let sources: ExtOutputImageCaptureSourceManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
        let manager: ExtImageCopyCaptureManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
        let mut app = App::default();
        if plan == Plan::CaptureDuringSecret {
            map_window(&globals, &mut queue, &mut app, &shm);
        }
        let source = sources.create_source(&output, &handle, ());
        let session = manager.create_session(&source, Options::empty(), &handle, ());
        while !app.done {
            let _dispatched = queue.roundtrip(&mut app).unwrap();
        }
        if !matches!(plan, Plan::Capture { .. }) {
            signal.send(()).unwrap();
            resume.recv().unwrap();
        }
        if plan == Plan::AwaitStop {
            while !app.stopped {
                let _dispatched = queue.roundtrip(&mut app).unwrap();
            }
        }
        let grow = if let Plan::Capture { grow } = plan {
            grow
        } else {
            (0, 0)
        };
        let first_pixel = capture(&mut queue, &mut app, &shm, &session, grow);
        Report {
            size: app.size,
            formats: app.formats,
            stopped: app.stopped,
            ready: app.outcome == Some(Ok(())),
            failed: app.outcome.and_then(Result::err),
            first_pixel,
        }
    }

    struct EmptyProvider;

    impl SecretProvider for EmptyProvider {
        fn lookup(&mut self, _: &SecretReference) -> Result<SecretValue, SecretProviderUnavailable> {
            Err(SecretProviderUnavailable)
        }
    }

    /// Runs one client against a headless Tehuti; `between` runs once when the client signals.
    fn run(plan: Plan, between: impl FnOnce(&mut Compositor) + 'static) -> Report {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let colour = Rc::new(RefCell::new(image::Rgba([10, 20, 30, 255])));
        compositor.set_capture_source(&colour, |colour, _, _| {
            Ok(image::RgbaImage::from_pixel(1280, 720, *colour))
        });
        compositor.set_secret_provider(Box::new(EmptyProvider));
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (signal, signalled) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, plan, &signal, &resumed));
        let mut between = Some(between);
        let started = Instant::now();
        while !thread.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(20), "client timed out");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            compositor.space.refresh();
            if signalled.try_recv().is_ok() {
                if let Some(between) = between.take() {
                    between(&mut compositor);
                }
                resume.send(()).unwrap();
            }
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        thread.join().unwrap()
    }

    #[test]
    fn client_captures_the_headless_output_into_an_shm_buffer() {
        let report = run(Plan::Capture { grow: (0, 0) }, |_| {});
        assert_eq!(report.size, Some((1280, 720)), "buffer size is the output mode");
        assert!(report.formats.contains(&wl_shm::Format::Argb8888), "{report:?}");
        assert!(report.ready, "frame was not ready: {report:?}");
        assert_eq!(report.failed, None, "frame failed");
        assert_eq!(
            report.first_pixel,
            [30, 20, 10, 255],
            "Argb8888 stores blue first"
        );
    }

    /// Format and stride errors never reach the capture: `wl_shm` refuses those buffers first,
    /// and the properties above cover them in `encode`.
    #[test]
    fn a_buffer_of_another_size_fails_with_buffer_constraints() {
        for grow in [(1, 0), (0, 1)] {
            let report = run(Plan::Capture { grow }, |_| {});
            assert!(!report.ready, "mismatched buffer was filled: {report:?}");
            assert_eq!(
                report.failed,
                Some(FailureReason::BufferConstraints.into()),
                "{grow:?}"
            );
            assert_eq!(report.first_pixel, [0; 4], "nothing is written");
        }
    }

    #[test]
    fn capture_fails_without_pixels_while_a_secret_confirmation_is_on_screen() {
        let report = run(Plan::CaptureDuringSecret, |compositor| {
            assert!(
                compositor.request_secret_delivery(SecretReference {
                    entry: "example".into()
                }),
                "secret request was refused"
            );
            assert!(
                compositor.secret_confirmation_text().is_some(),
                "confirmation not shown"
            );
        });
        assert!(!report.ready, "frame was captured during a secret confirmation");
        assert_eq!(
            report.failed,
            Some(FailureReason::Unknown.into()),
            "refusal hides its reason"
        );
        assert_eq!(report.first_pixel, [0; 4], "nothing is written");
    }

    #[test]
    fn sessions_stop_when_the_output_changes_mode() {
        let report = run(Plan::AwaitStop, |compositor| {
            let output = compositor.output.clone().unwrap();
            scale::apply(
                &output,
                Mode {
                    size: (1024, 768).into(),
                    refresh: 60_000,
                },
                1,
            );
            compositor.fill_output();
        });
        assert!(report.stopped, "session was not stopped: {report:?}");
        assert_eq!(
            report.failed,
            Some(FailureReason::Stopped.into()),
            "stale frames fail as stopped"
        );
    }
}
