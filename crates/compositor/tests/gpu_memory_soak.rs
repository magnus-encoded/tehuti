//! GPU memory stays bounded while windows are created, resized and destroyed.

#[cfg(test)]
mod gpu_memory_soak {
    use core::{cell::RefCell, time::Duration};
    use std::{
        io::Write,
        os::{fd::AsFd, unix::net::UnixStream},
        rc::Rc,
        sync::{Arc, mpsc},
        thread,
        time::Instant,
    };

    use smithay::{
        backend::{
            allocator::{
                Allocator, Buffer, Fourcc, Modifier,
                dmabuf::AsDmabuf,
                gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            },
            drm::DrmNode,
            renderer::{
                Bind, Frame, ImportDma, Offscreen, Renderer,
                damage::OutputDamageTracker,
                gles::{GlesRenderbuffer, GlesRenderer},
                multigpu::{
                    ApiDevice, GraphicsApi,
                    gbm::{GbmGlesBackend, GbmGlesDevice},
                },
            },
        },
        reexports::wayland_server::Display,
        utils::{DeviceFd, Rectangle, Transform},
    };
    use tehuti::{
        overlays::Overlays,
        state::{ClientState, Compositor},
    };
    use tehuti_core::layout::Command;
    use wayland_client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_buffer::WlBuffer,
            wl_compositor::WlCompositor,
            wl_registry::WlRegistry,
            wl_shm::{self, WlShm},
            wl_shm_pool::WlShmPool,
            wl_surface::WlSurface,
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
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);
    delegate_noop!(App: ignore ZwpLinuxDmabufV1);
    delegate_noop!(App: ignore ZwpLinuxBufferParamsV1);

    /// Returns modifiers the test renderer can paint into.
    fn render_modifiers(renderer: &GlesRenderer) -> Vec<Modifier> {
        renderer
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .filter(|format| format.code == Fourcc::Argb8888)
            .map(|format| format.modifier)
            .collect()
    }

    /// Allocates a fresh GPU buffer for every frame, as a resizing GL client does.
    fn draw_dmabuf(
        gbm: &mut GbmAllocator<DeviceFd>,
        dmabuf: &ZwpLinuxDmabufV1,
        surface: &WlSurface,
        handle: &QueueHandle<App>,
        size: (i32, i32),
        renderer: &mut GlesRenderer,
    ) {
        let modifiers = render_modifiers(renderer);
        let buffer_object = gbm
            .create_buffer(
                u32::try_from(size.0).unwrap(),
                u32::try_from(size.1).unwrap(),
                Fourcc::Argb8888,
                &modifiers,
            )
            .unwrap();
        let mut exported = buffer_object.export().unwrap();
        let mut target = renderer.bind(&mut exported).unwrap();
        {
            let mut frame = renderer
                .render(&mut target, size.into(), Transform::Normal)
                .unwrap();
            frame
                .clear([1.0, 0.0, 0.0, 1.0].into(), &[Rectangle::from_size(size.into())])
                .unwrap();
            frame
                .clear(
                    [0.0, 0.0, 1.0, 1.0].into(),
                    &[Rectangle::from_size((size.0, size.1 / 2).into())],
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        drop(target);
        let params = dmabuf.create_params(handle, ());
        let modifier = u64::from(exported.format().modifier);
        for (plane, ((plane_fd, offset), stride)) in exported
            .handles()
            .zip(exported.offsets())
            .zip(exported.strides())
            .enumerate()
        {
            params.add(
                plane_fd,
                u32::try_from(plane).unwrap(),
                offset,
                stride,
                u32::try_from(modifier >> 32).unwrap(),
                u32::try_from(modifier & 0xffff_ffff).unwrap(),
            );
        }
        let buffer = params.create_immed(
            size.0,
            size.1,
            u32::from_le_bytes(*b"AR24"),
            zwp_linux_buffer_params_v1::Flags::empty(),
            handle,
            (),
        );
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        buffer.destroy();
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "buffer sizes are small configured window sizes"
    )]
    fn draw(shm: &WlShm, surface: &WlSurface, handle: &QueueHandle<App>, size: (i32, i32)) {
        let bytes = size.0 * size.1 * 4;
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![200; usize::try_from(bytes).unwrap()])
            .unwrap();
        let pool = shm.create_pool(file.as_fd(), bytes, handle, ());
        let buffer = pool.create_buffer(
            0,
            size.0,
            size.1,
            size.0 * 4,
            wl_shm::Format::Argb8888,
            handle,
            (),
        );
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        buffer.destroy();
    }

    /// Opens `count` windows per round, redraws them on every configure, closes them on `close`.
    fn client(
        stream: UnixStream,
        count: usize,
        gpu_clients: bool,
        close: &mpsc::Receiver<bool>,
        closed: &mpsc::Sender<()>,
    ) {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let dmabuf: Option<ZwpLinuxDmabufV1> = gpu_clients.then(|| globals.bind(&handle, 3..=3, ()).unwrap());
        let mut gbm = gpu_clients.then(|| GbmAllocator::new(render_node(), GbmBufferFlags::RENDERING));
        let mut painter = gpu_clients.then(device);
        loop {
            let mut app = App::default();
            let mut windows = Vec::new();
            for index in 0..count {
                let surface = compositor.create_surface(&handle, ());
                let xdg = wm_base.get_xdg_surface(&surface, &handle, index);
                let toplevel = xdg.get_toplevel(&handle, index);
                toplevel.set_title("Test window".into());
                toplevel.set_app_id(format!("org.example.Soak{index}"));
                app.configured.push((300, 300));
                surface.commit();
                windows.push((surface, xdg, toplevel));
            }
            let finish = loop {
                for index in core::mem::take(&mut app.acked) {
                    match (&mut gbm, &dmabuf) {
                        (Some(gbm), Some(dmabuf)) => {
                            draw_dmabuf(
                                gbm,
                                dmabuf,
                                &windows[index].0,
                                &handle,
                                app.configured[index],
                                painter.as_mut().unwrap().renderer_mut(),
                            );
                        }
                        _ => draw(&shm, &windows[index].0, &handle, app.configured[index]),
                    }
                }
                let _dispatched = queue.roundtrip(&mut app).unwrap();
                match close.try_recv() {
                    Ok(finish) => break finish,
                    Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(1)),
                    Err(mpsc::TryRecvError::Disconnected) => break true,
                }
            };
            for (surface, xdg, toplevel) in windows {
                toplevel.destroy();
                xdg.destroy();
                surface.destroy();
            }
            let _dispatched = queue.roundtrip(&mut app).unwrap();
            closed.send(()).unwrap();
            if finish {
                return;
            }
        }
    }

    fn gpu_clients() -> bool {
        std::env::var_os("TEHUTI_SOAK_DMABUF").is_some()
    }

    fn render_node() -> GbmDevice<DeviceFd> {
        let node = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/dri/renderD128")
            .unwrap();
        GbmDevice::new(DeviceFd::from(std::os::fd::OwnedFd::from(node))).unwrap()
    }

    /// This process's GPU memory in MiB as `nvidia-smi` reports it, if it does.
    fn gpu_mib() -> Option<u64> {
        let output = std::process::Command::new("nvidia-smi").output().ok()?;
        let pid = std::process::id().to_string();
        String::from_utf8(output.stdout).ok()?.lines().find_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let _pid = fields.iter().position(|field| *field == pid)?;
            fields
                .iter()
                .rev()
                .find_map(|field| field.strip_suffix("MiB")?.parse().ok())
        })
    }

    type Device = GbmGlesDevice<GlesRenderer>;

    /// One renderer generation, as the DRM backend builds after start, hotplug or mode change.
    struct Gpu {
        device: Rc<RefCell<Device>>,
        target: GlesRenderbuffer,
        damage: OutputDamageTracker,
        overlays: Overlays,
    }

    fn device() -> Device {
        let gbm = render_node();
        let mut api: GbmGlesBackend<GlesRenderer, DeviceFd> = GbmGlesBackend::default();
        api.add_node(DrmNode::from_file(gbm.as_fd()).unwrap(), gbm)
            .unwrap();
        let mut devices = Vec::new();
        api.enumerate(&mut devices).unwrap();
        devices.pop().unwrap()
    }

    #[test]
    #[ignore = "needs a GPU render node; run with --ignored"]
    fn screenshot_contains_dmabuf_client_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut gpu = gpu(&display, &mut compositor);
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (close, closing) = mpsc::channel();
        let (closed, done) = mpsc::channel();
        let thread = thread::spawn(move || client(stream, 1, true, &closing, &closed));
        let started = Instant::now();
        while compositor.space.elements().next().is_none() {
            assert!(started.elapsed() < Duration::from_secs(5));
            frame(&mut gpu, &mut compositor, &mut display);
        }
        frames_for(
            &mut gpu,
            &mut compositor,
            &mut display,
            Duration::from_millis(100),
        );
        let window = compositor.space.elements().next().unwrap();
        let location = compositor.space.element_location(window).unwrap();
        let size = window.geometry().size;
        let upper = (
            u32::try_from(location.x + size.w / 2).unwrap(),
            u32::try_from(location.y + size.h / 4).unwrap(),
        );
        let lower = (upper.0, u32::try_from(location.y + size.h * 3 / 4).unwrap());
        let path = directory.path().join("shot.png");
        let mut driver =
            tehuti::driver::Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let _shot = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Shot { path: path.clone() },
            )
            .unwrap();
        close.send(true).unwrap();
        while done.try_recv().is_err() {
            frame(&mut gpu, &mut compositor, &mut display);
        }
        thread.join().unwrap();
        let image = image::open(path).unwrap().to_rgba8();
        let mode = compositor.output.as_ref().unwrap().current_mode().unwrap();
        assert_eq!(
            image.dimensions(),
            (
                u32::try_from(mode.size.w).unwrap(),
                u32::try_from(mode.size.h).unwrap()
            )
        );
        assert_eq!(*image.get_pixel(upper.0, upper.1), image::Rgba([0, 0, 255, 255]));
        assert_eq!(*image.get_pixel(lower.0, lower.1), image::Rgba([255, 0, 0, 255]));
    }

    /// Builds a renderer and routes client dmabuf imports to it, as the DRM backend does.
    fn gpu(display: &Display<Compositor>, compositor: &mut Compositor) -> Gpu {
        let device = Rc::new(RefCell::new(device()));
        compositor.set_capture_source(&device, |device, compositor, clock| {
            tehuti::rendering::capture_frame(device.renderer_mut(), compositor, clock)
        });
        let (formats, _) =
            tehuti::rendering::client_formats(device.borrow_mut().renderer_mut(), &display.handle());
        compositor
            .enable_dmabuf_on(
                &display.handle(),
                formats.iter().copied(),
                None,
                &device,
                |device, dmabuf| device.renderer_mut().import_dmabuf(dmabuf, None).is_ok(),
            )
            .unwrap();
        let output = compositor.output.clone().unwrap();
        let mode = output.current_mode().unwrap();
        let target = device
            .borrow_mut()
            .renderer_mut()
            .create_buffer(Fourcc::Abgr8888, (mode.size.w, mode.size.h).into())
            .unwrap();
        Gpu {
            device,
            target,
            damage: OutputDamageTracker::from_output(&output),
            overlays: Overlays::default(),
        }
    }

    fn frame(gpu: &mut Gpu, compositor: &mut Compositor, display: &mut Display<Compositor>) {
        let _dispatched = display.dispatch_clients(compositor).unwrap();
        compositor.space.refresh();
        display.flush_clients().unwrap();
        let output = compositor.output.clone().unwrap();
        let mut device = gpu.device.borrow_mut();
        let renderer = device.renderer_mut();
        let overlay = gpu
            .overlays
            .elements(renderer, &output, compositor, "00:00")
            .unwrap();
        let mut target = renderer.bind(&mut gpu.target).unwrap();
        tehuti::wallpaper::render_output_at_age(
            &output,
            renderer,
            &mut target,
            compositor,
            &overlay.elements,
            &mut gpu.damage,
            0,
        )
        .unwrap()
        .sync
        .wait()
        .unwrap();
        drop(target);
        drop(device);
        compositor.space.elements().for_each(|window| {
            window.send_frame(&output, Duration::ZERO, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        });
        thread::sleep(Duration::from_millis(2));
    }

    fn frames_for(
        gpu: &mut Gpu,
        compositor: &mut Compositor,
        display: &mut Display<Compositor>,
        duration: Duration,
    ) {
        let started = Instant::now();
        while started.elapsed() < duration {
            frame(gpu, compositor, display);
        }
    }

    /// Opens, resizes and closes three windows per round, after replacing the renderer once as a hotplug does.
    ///
    /// `TEHUTI_SOAK_DMABUF=1` makes the windows GPU clients; `TEHUTI_SOAK_ROUNDS` sets the length.
    #[test]
    #[expect(clippy::print_stderr, reason = "the hand-run soak reports each sample")]
    #[ignore = "needs a GPU render node and nvidia-smi; run with --ignored"]
    fn window_cycles_keep_compositor_gpu_memory_bounded() {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut gpu = gpu(&display, &mut compositor);
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (close, closing) = mpsc::channel();
        let (closed, done) = mpsc::channel();
        let rounds: usize = std::env::var("TEHUTI_SOAK_ROUNDS")
            .ok()
            .and_then(|rounds| rounds.parse().ok())
            .unwrap_or(60);
        let thread = thread::spawn(move || client(stream, 3, gpu_clients(), &closing, &closed));
        let mut samples = Vec::new();
        for round in 0..rounds {
            if round == 1 {
                let replaced = core::mem::replace(&mut gpu, self::gpu(&display, &mut compositor));
                drop(replaced);
            }
            frames_for(
                &mut gpu,
                &mut compositor,
                &mut display,
                Duration::from_millis(150),
            );
            for _ in 0..3 {
                compositor.apply_layout_command(Command::SwitchPresetColumnWidth);
                frames_for(&mut gpu, &mut compositor, &mut display, Duration::from_millis(60));
            }
            close.send(round + 1 == rounds).unwrap();
            while done.try_recv().is_err() {
                frame(&mut gpu, &mut compositor, &mut display);
            }
            for _ in 0..10 {
                frame(&mut gpu, &mut compositor, &mut display);
            }
            if let Some(mib) = gpu_mib() {
                eprintln!("round {round}: {mib} MiB");
                samples.push(mib);
            }
        }
        thread.join().unwrap();
        let (Some(first), Some(last)) = (samples.get(rounds / 4), samples.last()) else {
            panic!("nvidia-smi reported no GPU memory for this process");
        };
        assert!(
            last.saturating_sub(*first) < 16,
            "GPU memory grew from {first} MiB to {last} MiB over {rounds} window cycles: {samples:?}"
        );
        drop(gpu);
    }
}
