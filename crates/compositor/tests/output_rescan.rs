//! A connector rescan that finds the same output keeps its `wl_output` global alive.

#[cfg(test)]
mod output_rescan {
    use core::time::Duration;
    use std::{os::unix::net::UnixStream, sync::Arc, thread, time::Instant};

    use smithay::{output::Mode, reexports::wayland_server::Display};
    use tehuti::{
        drm_hotplug::publish_output,
        state::{ClientState, Compositor},
    };
    use wayland_client::{
        Connection, Dispatch, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_output::{self, WlOutput},
            wl_registry::{self, WlRegistry},
        },
    };

    #[derive(Default)]
    struct App {
        removed: usize,
        modes: Vec<(i32, i32)>,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for App {
        fn event(
            app: &mut Self,
            _: &WlRegistry,
            event: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if matches!(event, wl_registry::Event::GlobalRemove { .. }) {
                app.removed = app.removed.saturating_add(1);
            }
        }
    }

    impl Dispatch<WlOutput, ()> for App {
        fn event(
            app: &mut Self,
            _: &WlOutput,
            event: wl_output::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_output::Event::Mode { width, height, .. } = event {
                app.modes.push((width, height));
            }
        }
    }

    fn mode(width: i32, height: i32) -> Mode {
        Mode {
            size: (width, height).into(),
            refresh: 60_000,
        }
    }

    fn pump(display: &mut Display<Compositor>, compositor: &mut Compositor, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() && start.elapsed() < Duration::from_secs(5) {
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn rescanning_the_same_output_never_removes_its_global_from_clients() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let first = publish_output(&display.handle(), None, mode(1920, 1080), 1);
        let (client_end, server_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let (proceed, wait) = std::sync::mpsc::channel::<()>();
        let (seen, seen_rx) = std::sync::mpsc::channel::<usize>();
        let thread = thread::spawn(move || {
            let connection = Connection::from_socket(client_end).unwrap();
            let (_globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
            let mut app = App::default();
            let _events = queue.roundtrip(&mut app).unwrap();
            seen.send(0).unwrap();
            wait.recv().unwrap();
            let _later = queue.roundtrip(&mut app).unwrap();
            seen.send(app.removed).unwrap();
        });
        pump(&mut display, &mut compositor, || seen_rx.try_recv().is_ok());
        let second = publish_output(&display.handle(), Some(first), mode(1920, 1080), 1);
        proceed.send(()).unwrap();
        let mut removed = None;
        pump(&mut display, &mut compositor, || {
            removed = seen_rx.try_recv().ok();
            removed.is_some()
        });
        thread.join().unwrap();
        drop(second);
        assert_eq!(removed, Some(0), "clients saw the output global disappear");
    }

    #[test]
    fn rescanning_a_different_monitor_leaves_only_its_mode_on_the_output() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let first = publish_output(&display.handle(), None, mode(1920, 1080), 1);
        let second = publish_output(&display.handle(), Some(first), mode(2560, 1440), 1);
        let (client_end, server_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let thread = thread::spawn(move || {
            let connection = Connection::from_socket(client_end).unwrap();
            let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
            let mut app = App::default();
            let _output: WlOutput = globals.bind(&queue.handle(), 1..=4, ()).unwrap();
            let _first = queue.roundtrip(&mut app).unwrap();
            let _second = queue.roundtrip(&mut app).unwrap();
            app.modes
        });
        pump(&mut display, &mut compositor, || thread.is_finished());
        let modes = thread.join().unwrap();
        drop(second);
        assert_eq!(
            modes,
            vec![(2560, 1440)],
            "clients saw modes of the previous monitor"
        );
    }
}
