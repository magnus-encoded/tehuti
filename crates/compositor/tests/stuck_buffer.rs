//! A client whose buffer never becomes ready cannot stall the output.

#[cfg(test)]
mod stuck_buffer {
    use core::time::Duration;
    use std::{
        io::Write,
        os::{fd::AsFd, unix::net::UnixStream},
        sync::{Arc, mpsc},
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

    fn client(
        stream: UnixStream,
        pipe_write: &mpsc::Sender<std::fs::File>,
        ready: &mpsc::Sender<()>,
        stop: &mpsc::Receiver<()>,
    ) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&handle, 3..=3, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg.get_toplevel(&handle, ());
        toplevel.set_title("Test window".into());
        toplevel.set_app_id(APP_ID.into());
        surface.commit();
        let mut app = App::default();
        while !app.acked {
            let _configured = queue.roundtrip(&mut app).unwrap();
        }
        let (width, height) = app.size.unwrap();
        let stride = u32::try_from(width).unwrap().checked_mul(4).unwrap();
        // The read end never becomes readable until the test writes to the other end.
        let (read, write) = rustix::pipe::pipe().unwrap();
        pipe_write.send(std::fs::File::from(write)).unwrap();
        let params = dmabuf.create_params(&handle, ());
        params.add(read.as_fd(), 0, 0, stride, 0, 0);
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

    #[test]
    fn a_buffer_that_never_becomes_ready_leaves_the_rest_of_the_output_drawing() {
        let directory = tempfile::tempdir().unwrap();
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        compositor
            .enable_dmabuf(
                &display.handle(),
                [Format {
                    code: Fourcc::Argb8888,
                    modifier: Modifier::Linear,
                }],
                None,
                |_| true,
            )
            .unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (pipe_write, pipe_end) = mpsc::channel();
        let (ready, received) = mpsc::channel();
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, &pipe_write, &ready, &stopped));
        let started = Instant::now();
        while received.try_recv().is_err() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            pump(&mut display, &mut compositor);
        }
        for _ in 0..50 {
            pump(&mut display, &mut compositor);
            compositor.advance_time();
        }
        assert_eq!(compositor.buffers_waiting(), 1, "the unfinished buffer holds its commit");
        let path = directory.path().join("shot.png");
        let _shot = driver
            .execute(&mut compositor, Request::Shot { path: path.clone() })
            .unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        let bottom = image.height() - 8;
        assert_ne!(
            *image.get_pixel(image.width() / 2, bottom),
            image::Rgba([0, 0, 0, 255]),
            "the voice bar still renders while the client buffer is stuck"
        );
        let mut write = pipe_end.recv().unwrap();
        write.write_all(b"x").unwrap();
        for _ in 0..50 {
            pump(&mut display, &mut compositor);
            compositor.advance_time();
        }
        assert_eq!(compositor.buffers_waiting(), 0, "the commit goes through once the buffer is ready");
        stop.send(()).unwrap();
        while !thread.is_finished() {
            pump(&mut display, &mut compositor);
        }
        thread.join().unwrap();
    }
}
