//! A real X11 client maps through Smithay without launching the compositor binary.
#[cfg(test)]
mod tests {
    use core::time::Duration;
    use smithay::reexports::{
        wayland_server::Display,
        x11rb::{
            self,
            connection::Connection,
            protocol::xproto::{AtomEnum, ConnectionExt, CreateWindowAux, PropMode, WindowClass},
        },
    };
    use std::{sync::mpsc, thread, time::Instant};
    use tehuti::{state::Compositor, xwayland::Runtime};

    #[test]
    fn x11_client_maps_into_the_tiling_layout() {
        if !installed() {
            return;
        }
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let mut runtime = Runtime::new().unwrap();
        runtime.dispatch(&display.handle(), &mut compositor);
        let name = runtime.display_name().unwrap().to_owned();
        let (ready, mapped) = mpsc::channel();
        let (finish, finished) = mpsc::channel();
        let client = thread::spawn(move || map_client(&name, &ready, &finished));
        let deadline = Instant::now().checked_add(Duration::from_secs(10)).unwrap();
        let mut client_mapped = false;
        while Instant::now() < deadline {
            runtime.dispatch(&display.handle(), &mut compositor);
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            display.flush_clients().unwrap();
            client_mapped |= mapped.try_recv().is_ok();
            if client_mapped
                && compositor
                    .voice_bar()
                    .segments()
                    .first()
                    .is_some_and(|segment| segment.text == "X11 test")
            {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            compositor.space.elements().any(|window| window
                .x11_surface()
                .is_some_and(|surface| surface.class() == "TestGame")),
            "WM_CLASS identifies the mapped application"
        );
        assert!(
            compositor
                .voice_bar()
                .segments()
                .first()
                .is_some_and(|segment| segment.text == "X11 test"),
            "focused X11 title reaches the bar"
        );
        let _sent = finish.send(());
        client.join().unwrap();
        assert!(client_mapped, "X11 client completes its map request");
        assert!(
            compositor.layout_snapshot().focused_window.is_some(),
            "X11 window enters the shared tiling layout"
        );
    }
    fn installed() -> bool {
        use std::io::Write;
        if std::process::Command::new("Xwayland")
            .arg("-version")
            .output()
            .is_ok()
        {
            return true;
        }
        let _reported = writeln!(
            std::io::stderr().lock(),
            "SKIP: Xwayland is not installed; X11 mapping test requires Xwayland"
        );
        false
    }

    fn map_client(name: &str, ready: &mpsc::Sender<()>, finished: &mpsc::Receiver<()>) {
        use x11rb::wrapper::ConnectionExt as _;
        let (connection, screen) = x11rb::connect(Some(name)).unwrap();
        let root = connection.setup().roots.get(screen).unwrap().root;
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                320,
                240,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"Claude: X11 test",
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                b"game\0TestGame\0",
            )
            .unwrap()
            .check()
            .unwrap();
        connection.map_window(window).unwrap().check().unwrap();
        connection.flush().unwrap();
        ready.send(()).unwrap();
        let _finished = finished.recv_timeout(Duration::from_secs(10));
    }
}
