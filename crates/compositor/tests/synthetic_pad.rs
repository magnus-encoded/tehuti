//! End to end without a display or devices: a synthetic pad press reaches a Wayland client as a key.

#[cfg(test)]
mod synthetic_pad {
    use core::time::Duration;
    use std::{
        os::unix::net::UnixStream,
        sync::{Arc, mpsc},
        thread,
        time::Instant,
    };

    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        input::{SyntheticPad, route},
        state::{ClientState, Compositor},
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event};
    use wayland_client::{
        Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_compositor::WlCompositor,
            wl_keyboard::{self, KeyState, WlKeyboard},
            wl_registry::WlRegistry,
            wl_seat::WlSeat,
            wl_surface::WlSurface,
        },
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::XdgToplevel,
        xdg_wm_base::{self, XdgWmBase},
    };

    /// `KEY_ENTER` from linux/input-event-codes.h, as `wl_keyboard.key` reports it.
    const KEY_ENTER: u32 = 28;

    #[derive(Debug, Default)]
    struct App {
        keys: Vec<(u32, KeyState)>,
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

    impl Dispatch<WlKeyboard, ()> for App {
        fn event(
            app: &mut Self,
            _: &WlKeyboard,
            event: wl_keyboard::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(state),
                ..
            } = event
            {
                app.keys.push((key, state));
            }
        }
    }

    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlSeat);
    delegate_noop!(App: ignore XdgToplevel);

    /// Opens one toplevel and returns every key it receives once it has seen one full key tap.
    fn run_client(stream: UnixStream, app_id: &str, ready: &mpsc::Sender<()>) -> Vec<(u32, KeyState)> {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let seat: WlSeat = globals.bind(&handle, 1..=1, ()).unwrap();
        let _keyboard = seat.get_keyboard(&handle, ());
        let surface = compositor.create_surface(&handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg_surface.get_toplevel(&handle, ());
        toplevel.set_app_id(if app_id == "test" { "other" } else { "test" }.into());
        surface.commit();
        let mut app = App::default();
        let _initial_dispatch = queue.roundtrip(&mut app).unwrap();
        toplevel.set_app_id(app_id.into());
        let _focus_dispatch = queue.roundtrip(&mut app).unwrap();
        ready.send(()).unwrap();
        while app.keys.len() < 2 {
            let _dispatched = queue.blocking_dispatch(&mut app).unwrap();
        }
        // Any further key the compositor sent arrives before this roundtrip completes.
        let _dispatched = queue.roundtrip(&mut app).unwrap();
        app.keys
    }

    #[test]
    fn changing_to_an_unmatched_app_id_uses_global_and_fallback_routing() {
        check_routing("other", KEY_ENTER);
    }

    #[test]
    fn changing_to_a_matching_app_id_sends_tab_from_a_synthetic_pad() {
        check_routing("test", 15);
    }

    fn check_routing(app_id: &'static str, expected_key: u32) {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let directory =
            std::env::temp_dir().join(format!("tehuti-wayland-context-{}-{app_id}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("context.toml"),
            "app_id = 'test'\n[buttons.east]\nkey = 'tab'",
        )
        .unwrap();
        compositor.set_contexts(tehuti::contexts::load(&directory).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let client = thread::spawn(move || run_client(client_end, app_id, &ready_sender));

        let mut pad = SyntheticPad::default();
        for name in [ButtonName::East, ButtonName::Start, ButtonName::South] {
            pad.push(Event::Button {
                name,
                state: ButtonState::Pressed,
            });
            pad.push(Event::Button {
                name,
                state: ButtonState::Released,
            });
        }
        let mut pressed = false;
        let started = Instant::now();
        while !client.is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "client never received a key"
            );
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            if !pressed && ready_receiver.try_recv().is_ok() {
                route(&mut compositor, &mut pad).unwrap();
                pressed = true;
            }
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            client.join().unwrap(),
            if expected_key == KEY_ENTER {
                vec![(KEY_ENTER, KeyState::Pressed), (KEY_ENTER, KeyState::Released)]
            } else {
                vec![
                    (expected_key, KeyState::Pressed),
                    (expected_key, KeyState::Released),
                    (KEY_ENTER, KeyState::Pressed),
                    (KEY_ENTER, KeyState::Released),
                ]
            },
            "context and global actions reach the client as complete taps"
        );
    }
}
