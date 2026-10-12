//! End to end without a display or devices: a synthetic pad press reaches a Wayland client as a key.

#[cfg(test)]
mod synthetic_pad {
    use core::time::Duration;
    use std::{os::unix::net::UnixStream, sync::Arc, thread, time::Instant};

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
    fn run_client(stream: UnixStream) -> Vec<(u32, KeyState)> {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let seat: WlSeat = globals.bind(&handle, 1..=1, ()).unwrap();
        let _keyboard = seat.get_keyboard(&handle, ());
        let surface = compositor.create_surface(&handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let _toplevel = xdg_surface.get_toplevel(&handle, ());
        surface.commit();
        let mut app = App::default();
        while app.keys.len() < 2 {
            let _dispatched = queue.blocking_dispatch(&mut app).unwrap();
        }
        // Any further key the compositor sent arrives before this roundtrip completes.
        let _dispatched = queue.roundtrip(&mut app).unwrap();
        app.keys
    }

    #[test]
    fn pressing_a_on_a_synthetic_pad_sends_enter_to_the_focused_client() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let client = thread::spawn(move || run_client(client_end));

        let mut pad = SyntheticPad::default();
        for name in [
            ButtonName::East,
            ButtonName::Start,
            ButtonName::South,
            ButtonName::East,
        ] {
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
        let deadline = Instant::now() + Duration::from_secs(10);
        while !client.is_finished() {
            assert!(Instant::now() < deadline, "client never received a key");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            if !pressed && compositor.space.elements().next().is_some() {
                route(&mut compositor, &mut pad).unwrap();
                pressed = true;
            }
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            client.join().unwrap(),
            vec![(KEY_ENTER, KeyState::Pressed), (KEY_ENTER, KeyState::Released)],
            "only A reaches the client, as one Enter tap"
        );
    }
}
