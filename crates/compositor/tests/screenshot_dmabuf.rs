//! A screenshot keeps rendering when a window's buffer cannot be imported.

#[cfg(test)]
mod screenshot_dmabuf {
    use core::time::Duration;
    use std::{
        io::Write,
        os::{fd::AsFd, unix::net::UnixStream},
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Instant,
    };

    use smithay::{
        backend::allocator::{Format, Fourcc, Modifier},
        reexports::wayland_server::Display,
    };
    use tehuti::{
        driver::{Driver, Request},
        state::{ClientState, Compositor},
    };
    use wayland_client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_surface::WlSurface,
        },
    };
    use wayland_protocols::{
        wp::linux_dmabuf::zv1::client::{
            zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
            zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        },
        xdg::shell::client::{
            xdg_surface::{self, XdgSurface},
            xdg_toplevel::{self, XdgToplevel},
            xdg_wm_base::{self, XdgWmBase},
        },
    };

    const APP_ID: &str = "org.example.Viewer";
    /// An Intel render-compression modifier: colour plane plus auxiliary plane.
    const Y_TILED_CCS: u64 = 0x0100_0000_0000_0004;

    #[derive(Default)]
    struct App {
        size: Option<(i32, i32)>,
        acked: bool,
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
            if let xdg_toplevel::Event::Configure { width, height, .. } = event {
                app.size = Some((
                    if width > 0 { width } else { 300 },
                    if height > 0 { height } else { 300 },
                ));
            }
        }
    }
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlBuffer);
    delegate_noop!(App: ignore ZwpLinuxDmabufV1);
    delegate_noop!(App: ignore ZwpLinuxBufferParamsV1);

    fn plane(bytes: usize) -> std::fs::File {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![255; bytes]).unwrap();
        file
    }

    fn client(stream: UnixStream, ready: &mpsc::Sender<()>, stop: &mpsc::Receiver<()>) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&handle, 3..=3, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg.get_toplevel(&handle, ());
        toplevel.set_app_id(APP_ID.into());
        surface.commit();
        let mut app = App::default();
        while !app.acked {
            let _configured = queue.roundtrip(&mut app).unwrap();
        }
        let (width, height) = app.size.unwrap();
        let stride = u32::try_from(width).unwrap().checked_mul(4).unwrap();
        let rows = usize::try_from(height).unwrap();
        let colour = plane(usize::try_from(stride).unwrap().checked_mul(rows).unwrap());
        let auxiliary = plane(rows.checked_mul(64).unwrap());
        let params = dmabuf.create_params(&handle, ());
        let modifier_hi = u32::try_from(Y_TILED_CCS >> 32).unwrap();
        let modifier_lo = u32::try_from(Y_TILED_CCS & 0xffff_ffff).unwrap();
        params.add(colour.as_fd(), 0, 0, stride, modifier_hi, modifier_lo);
        params.add(auxiliary.as_fd(), 1, 0, 64, modifier_hi, modifier_lo);
        let buffer = params.create_immed(
            width,
            height,
            u32::from_le_bytes(*b"AR24"),
            zwp_linux_buffer_params_v1::Flags::empty(),
            &handle,
            (),
        );
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        let _committed = queue.roundtrip(&mut app).unwrap();
        ready.send(()).unwrap();
        while stop.try_recv().is_err() {
            let _dispatched = queue.roundtrip(&mut app).unwrap();
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn pump(display: &mut Display<Compositor>, compositor: &mut Compositor) {
        let _dispatched = display.dispatch_clients(compositor).unwrap();
        compositor.space.refresh();
        display.flush_clients().unwrap();
        thread::sleep(Duration::from_millis(1));
    }

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);

    impl Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|error| std::io::Error::other(error.to_string()))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Saves a screenshot and returns what was logged while taking it.
    fn shot_with_log(driver: &mut Driver, compositor: &mut Compositor, path: &std::path::Path) -> String {
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let _shot = driver
                .execute(compositor, Request::Shot { path: path.into() })
                .unwrap();
        });
        String::from_utf8(log.0.lock().unwrap().clone()).unwrap()
    }

    fn compositor_importing_ccs(display: &Display<Compositor>) -> Compositor {
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        compositor
            .enable_dmabuf(
                &display.handle(),
                [Format {
                    code: Fourcc::Argb8888,
                    modifier: Modifier::from(Y_TILED_CCS),
                }],
                None,
                |_| true,
            )
            .unwrap();
        compositor
    }

    #[test]
    fn a_window_whose_buffer_cannot_be_imported_is_captured_as_a_placeholder_and_warned_once() {
        let directory = tempfile::tempdir().unwrap();
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = compositor_importing_ccs(&display);
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (ready, received) = mpsc::channel();
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, &ready, &stopped));
        let started = Instant::now();
        while received.try_recv().is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            pump(&mut display, &mut compositor);
        }
        for _ in 0..50 {
            pump(&mut display, &mut compositor);
        }
        let window = compositor.space.elements().next().unwrap();
        let location = compositor.space.element_location(window).unwrap();
        let size = window.geometry().size;
        let centre = (
            u32::try_from(location.x + size.w / 2).unwrap(),
            u32::try_from(location.y + size.h / 2).unwrap(),
        );
        let path = directory.path().join("shot.png");
        let mut log = shot_with_log(&mut driver, &mut compositor, &path);
        log.push_str(&shot_with_log(&mut driver, &mut compositor, &path));
        stop.send(()).unwrap();
        while !thread.is_finished() {
            pump(&mut display, &mut compositor);
        }
        thread.join().unwrap();

        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!(
            *image.get_pixel(centre.0, centre.1),
            image::Rgba(tehuti::import_placeholder::RGBA),
            "the window area shows the placeholder"
        );
        let bottom = image.height() - 8;
        assert_ne!(
            *image.get_pixel(image.width() / 2, bottom),
            image::Rgba([0, 0, 0, 255]),
            "the voice bar still renders below the window"
        );
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("WARN") && line.contains(APP_ID))
                .count(),
            1,
            "two captures warn once, naming the window: {log}"
        );
    }
}
