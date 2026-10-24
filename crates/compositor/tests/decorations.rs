//! The decorations setting: clients keep their title bars unless it says `none`.

#[cfg(test)]
mod decorations {
    use core::time::Duration;
    use std::{ffi::OsStr, os::unix::net::UnixStream, sync::Arc, thread, time::Instant};

    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        decorations::{Decorations, choose, read_file},
        state::{ClientState, Compositor},
    };
    use wayland_client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_surface::WlSurface},
    };
    use wayland_protocols::xdg::{
        decoration::zv1::client::{
            zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
            zxdg_toplevel_decoration_v1::{self, Mode, ZxdgToplevelDecorationV1},
        },
        shell::client::{
            xdg_surface::{self, XdgSurface},
            xdg_toplevel::XdgToplevel,
            xdg_wm_base::{self, XdgWmBase},
        },
    };

    #[test]
    fn missing_or_empty_settings_keep_client_decorations() {
        assert_eq!(choose(None, None), Ok(Decorations::Client));
        assert_eq!(choose(Some(OsStr::new("")), Some("\n")), Ok(Decorations::Client));
    }

    #[test]
    fn the_file_chooses_when_the_variable_is_unset() {
        assert_eq!(choose(None, Some("none\n")), Ok(Decorations::None));
        assert_eq!(choose(None, Some(" None ")), Ok(Decorations::None));
        assert_eq!(choose(None, Some("client")), Ok(Decorations::Client));
    }

    #[test]
    fn the_variable_overrides_the_file() {
        assert_eq!(
            choose(Some(OsStr::new("none")), Some("client")),
            Ok(Decorations::None)
        );
        assert_eq!(
            choose(Some(OsStr::new("client")), Some("none")),
            Ok(Decorations::Client)
        );
    }

    #[test]
    fn unknown_values_warn_and_keep_client_decorations() {
        assert!(matches!(choose(None, Some("off")), Err((Decorations::Client, _))));
        assert!(matches!(
            choose(Some(OsStr::new("hidden")), Some("none")),
            Err((Decorations::Client, _))
        ));
    }

    #[test]
    fn a_missing_file_reads_as_unset() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("decorations");
        assert_eq!(read_file(&path)?, None);
        std::fs::write(&path, "none\n")?;
        assert_eq!(read_file(&path)?.as_deref(), Some("none\n"));
        Ok(())
    }

    #[derive(Default)]
    struct App {
        modes: Vec<Mode>,
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
    impl Dispatch<ZxdgToplevelDecorationV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ZxdgToplevelDecorationV1,
            event: zxdg_toplevel_decoration_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zxdg_toplevel_decoration_v1::Event::Configure {
                mode: wayland_client::WEnum::Value(mode),
            } = event
            {
                app.modes.push(mode);
            }
        }
    }
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore XdgToplevel);
    delegate_noop!(App: ignore ZxdgDecorationManagerV1);

    /// The decoration modes a client asking for client-side mode receives, or `None` without the global.
    fn client(stream: UnixStream) -> Option<Vec<Mode>> {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let manager: ZxdgDecorationManagerV1 = globals.bind(&handle, 1..=1, ()).ok()?;
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg_surface.get_toplevel(&handle, ());
        let decoration = manager.get_toplevel_decoration(&toplevel, &handle, ());
        decoration.set_mode(Mode::ClientSide);
        surface.commit();
        let mut app = App::default();
        let _first = queue.roundtrip(&mut app).unwrap();
        let _second = queue.roundtrip(&mut app).unwrap();
        decoration.unset_mode();
        let _third = queue.roundtrip(&mut app).unwrap();
        Some(app.modes)
    }

    fn run(decorations: Decorations) -> Option<Vec<Mode>> {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_decorations(decorations);
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
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        thread.join().unwrap()
    }

    #[test]
    fn client_setting_advertises_no_decoration_manager() {
        assert_eq!(run(Decorations::Client), None);
    }

    #[test]
    fn none_setting_tells_every_client_to_drop_its_title_bar() {
        let modes = run(Decorations::None).expect("decoration manager advertised");
        assert!(!modes.is_empty(), "no decoration configure");
        assert!(modes.iter().all(|mode| *mode == Mode::ServerSide), "{modes:?}");
    }
}
