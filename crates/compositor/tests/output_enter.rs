//! A toplevel learns its output (and so its scale) before its first configure.

#[cfg(test)]
mod output_enter {
    use core::time::Duration;
    use std::{os::unix::net::UnixStream, sync::Arc, thread, time::Instant};

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
            wl_compositor::WlCompositor,
            wl_output::WlOutput,
            wl_registry::WlRegistry,
            wl_surface::{self, WlSurface},
        },
    };
    use wayland_protocols::wp::fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::{self, XdgToplevel},
        xdg_wm_base::{self, XdgWmBase},
    };

    #[derive(Default)]
    struct App {
        order: Vec<&'static str>,
        buffer_scale: Option<i32>,
        fractional: Option<u32>,
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
            if matches!(event, xdg_toplevel::Event::Configure { .. }) {
                app.order.push("configure");
            }
        }
    }
    impl Dispatch<WlSurface, ()> for App {
        fn event(
            app: &mut Self,
            _: &WlSurface,
            event: wl_surface::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if matches!(event, wl_surface::Event::Enter { .. }) {
                app.order.push("enter");
            }
            if matches!(event, wl_surface::Event::Leave { .. }) {
                app.order.push("leave");
            }
            if let wl_surface::Event::PreferredBufferScale { factor } = event {
                app.buffer_scale = Some(factor);
            }
        }
    }
    impl Dispatch<WpFractionalScaleV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &WpFractionalScaleV1,
            event: wp_fractional_scale_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
                app.fractional = Some(scale);
            }
        }
    }
    delegate_noop!(App: ignore WpFractionalScaleManagerV1);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlOutput);

    fn client(stream: UnixStream) -> App {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 6..=6, ()).unwrap();
        let manager: WpFractionalScaleManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let _output: WlOutput = globals.bind(&handle, 1..=4, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let _fractional = manager.get_fractional_scale(&surface, &handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let _toplevel = xdg_surface.get_toplevel(&handle, ());
        surface.commit();
        let mut app = App::default();
        let _first = queue.roundtrip(&mut app).unwrap();
        let _second = queue.roundtrip(&mut app).unwrap();
        app
    }

    #[test]
    fn the_surface_enters_its_output_before_the_first_configure() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
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
        let mode = Mode {
            size: (3840, 2160).into(),
            refresh: 60_000,
        };
        scale::apply(&output, mode, 2);
        compositor.space.map_output(&output, (0, 0));
        compositor.output = Some(output);
        compositor.fill_output();
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let thread = thread::spawn(move || client(stream));
        let started = Instant::now();
        while !thread.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            compositor.space.refresh();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        let app = thread.join().unwrap();
        let order = app.order;
        assert_eq!(app.buffer_scale, Some(2));
        assert_eq!(app.fractional, Some(240));
        assert_eq!(order.first(), Some(&"enter"), "{order:?}");
        assert!(!order.contains(&"leave"), "{order:?}");
        assert!(order.contains(&"configure"), "{order:?}");
    }
}
