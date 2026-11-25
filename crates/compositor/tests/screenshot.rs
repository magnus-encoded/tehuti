pub mod harness;

#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        driver::{Driver, Request},
        state::Compositor,
    };
    use tehuti_protocol::ButtonName;

    /// Runs `test` again in a child whose home, config and data directories are empty, so the
    /// process-wide wallpaper cache cannot pick up an installed wallpaper. True in the child.
    fn clean_environment(test: &str, wallpapers: Option<&[u8]>) -> bool {
        if std::env::var_os("SCREENSHOT_CLEAN").is_some() {
            return true;
        }
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        if let Some(png) = wallpapers {
            std::fs::create_dir_all(data.join("tehuti/wallpapers")).unwrap();
            std::fs::write(data.join("tehuti/wallpapers/apteribis.png"), png).unwrap();
        }
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test])
            .env("SCREENSHOT_CLEAN", "1")
            .env("HOME", directory.path())
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .env("XDG_DATA_HOME", data)
            .env("XDG_DATA_DIRS", directory.path().join("none"))
            .env_remove("TEHUTI_ALL_WALLPAPERS")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{test}: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        false
    }

    #[test]
    fn first_shot_shows_the_wallpaper() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(64, 36, image::Rgba([10, 120, 200, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        if !clean_environment("tests::first_shot_shows_the_wallpaper", Some(png.get_ref())) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let path = directory.path().join("first.png");
        let _shot = driver
            .execute(&mut compositor, Request::Shot { path: path.clone() })
            .unwrap();
        let shot = image::open(&path).unwrap().to_rgba8();
        assert_eq!(*shot.get_pixel(40, 50), image::Rgba([10, 120, 200, 255]));
    }

    #[test]
    fn empty_nested_desktop_shot_has_no_favourite_icons() {
        if !clean_environment("tests::empty_nested_desktop_shot_has_no_favourite_icons", None) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let path = directory.path().join("empty-desktop.png");
        let _shot = driver
            .execute(&mut compositor, Request::Shot { path: path.clone() })
            .unwrap();
        let shot = image::open(&path).unwrap().to_rgba8();
        assert_eq!(*shot.get_pixel(40, 50), image::Rgba([0, 0, 0, 255]));
        if let Some(destination) = std::env::var_os("TEHUTI_EMPTY_DESKTOP_SHOT") {
            let _bytes = std::fs::copy(path, destination).unwrap();
        }
    }

    #[test]
    fn screenshot_uses_the_current_output_capture_source() {
        use core::cell::RefCell;
        use std::rc::Rc;
        if !clean_environment("tests::screenshot_uses_the_current_output_capture_source", None) {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let first = Rc::new(RefCell::new(image::Rgba([255, 0, 0, 255])));
        let second = Rc::new(RefCell::new(image::Rgba([0, 255, 0, 255])));
        let path = directory.path().join("shot.png");
        for device in [&first, &second] {
            compositor.set_capture_source(device, |colour, _, _| {
                Ok(image::RgbaImage::from_pixel(1280, 720, *colour))
            });
            let _shot = driver
                .execute(&mut compositor, Request::Shot { path: path.clone() })
                .unwrap();
            let image = image::open(&path).unwrap().to_rgba8();
            assert_eq!(*image.get_pixel(640, 360), *device.borrow());
        }
        assert_eq!(Rc::strong_count(&first), 1);
        assert_eq!(Rc::strong_count(&second), 1);
        let busy = second.borrow_mut();
        assert!(
            driver
                .execute(&mut compositor, Request::Shot { path: path.clone() })
                .is_err()
        );
        drop(busy);
        drop(second);
        assert!(
            driver
                .execute(&mut compositor, Request::Shot { path: path.clone() })
                .is_err()
        );
        compositor.clear_capture_source();
        let _shot = driver
            .execute(&mut compositor, Request::Shot { path: path.clone() })
            .unwrap();
        assert_eq!(
            *image::open(path).unwrap().to_rgba8().get_pixel(640, 360),
            image::Rgba([0, 0, 0, 255])
        );
    }

    #[test]
    fn screenshot_menu_saves_the_frame_without_the_menu() {
        if std::env::var_os("SCREENSHOT_PROBE").is_none() {
            for mode in ["explicit", "fallback", "configured", "failure"] {
                isolated_probe(mode);
            }
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        compositor.set_contexts(vec![
            tehuti_protocol::parse_context(
                "app_id = '*'\nbuttons = {}\n[speech.frame]\nnotify = 'KNOWN FRAME'",
            )
            .unwrap(),
        ]);
        let _notice = driver
            .execute(&mut compositor, Request::Say { text: "frame".into() })
            .unwrap();
        let before = directory.path().join("before.png");
        let _capture = driver
            .execute(&mut compositor, Request::Shot { path: before.clone() })
            .unwrap();
        select_screenshot(&mut driver, &mut compositor);
        let mode = std::env::var("SCREENSHOT_PROBE").unwrap();
        if mode == "failure" {
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .starts_with("SCREENSHOT FAILED:")
            );
            assert_eq!(
                driver.execute(&mut compositor, Request::Scene).unwrap()["overlays"]["menu"],
                false
            );
            assert!(!compositor.quit_requested());
            return;
        }
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let folder = home
            .join(match mode.as_str() {
                "configured" => "Images",
                "explicit" => "Chosen",
                _ => "Pictures",
            })
            .join("Screenshots");
        let files: Vec<_> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        assert_frame(&files[0], &before);
        assert!(
            compositor
                .visible_notice_text()
                .unwrap()
                .contains(&files[0].display().to_string())
        );
    }

    fn assert_frame(path: &std::path::Path, before: &std::path::Path) {
        let actual = image::open(path).unwrap().to_rgba8();
        let expected = image::open(before).unwrap().to_rgba8();
        assert_eq!(actual.dimensions(), (1280, 720));
        assert_eq!(*actual.get_pixel(640, 360), image::Rgba([0, 0, 0, 255]));
        assert_eq!(*actual.get_pixel(16, 16), image::Rgba([32, 32, 32, 255]));
        assert_eq!(*actual.get_pixel(16, 703), image::Rgba([47, 65, 88, 255]));
        assert_eq!(
            image::imageops::crop_imm(&actual, 0, 0, 1280, 650).to_image(),
            image::imageops::crop_imm(&expected, 0, 0, 1280, 650).to_image()
        );
        assert_eq!(path.extension().unwrap(), "png");
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Screenshot-")
        );
    }

    fn isolated_probe(mode: &str) {
        let directory = tempfile::tempdir().unwrap();
        let pictures = directory.path().join("Chosen");
        let config = directory.path().join("config");
        std::fs::create_dir(&config).unwrap();
        if mode == "configured" {
            std::fs::write(
                config.join("user-dirs.dirs"),
                "XDG_PICTURES_DIR=\"$HOME/Images\"\n",
            )
            .unwrap();
        }
        if mode == "failure" {
            std::fs::write(&pictures, "occupied").unwrap();
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        let _configured = command
            .args([
                "--exact",
                "tests::screenshot_menu_saves_the_frame_without_the_menu",
            ])
            .env("SCREENSHOT_PROBE", mode)
            .env("HOME", directory.path())
            .env("XDG_CONFIG_HOME", config)
            .env("XDG_DATA_HOME", directory.path().join("data"))
            .env("XDG_DATA_DIRS", directory.path().join("none"))
            .env("TEHUTI_TEST_SIZE", "1280x720")
            .env_remove("XDG_PICTURES_DIR");
        if matches!(mode, "explicit" | "failure") {
            let _pictures = command.env("XDG_PICTURES_DIR", pictures);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{mode}: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn select_screenshot(driver: &mut Driver, compositor: &mut Compositor) {
        let _held = driver
            .execute(
                compositor,
                Request::Hold {
                    button: ButtonName::Guide,
                },
            )
            .unwrap();
        let _elapsed = driver
            .execute(compositor, Request::Advance { milliseconds: 500 })
            .unwrap();
        let _released = driver
            .execute(
                compositor,
                Request::Release {
                    button: ButtonName::Guide,
                },
            )
            .unwrap();
        // Screenshot is second from the bottom; Reboot and Shut Down may be hidden above it.
        let from_bottom = tehuti_core::menu::ITEMS
            .iter()
            .rev()
            .position(|item| item.key() == "screenshot")
            .expect("Screenshot menu item");
        for _ in 0..=from_bottom {
            let _moved = driver
                .execute(
                    compositor,
                    Request::Press {
                        button: ButtonName::DpadUp,
                    },
                )
                .unwrap();
        }
        let _selected = driver
            .execute(
                compositor,
                Request::Press {
                    button: ButtonName::South,
                },
            )
            .unwrap();
    }

    #[test]
    fn voice_and_pad_capture_commands_save_screen_and_window_screenshots() {
        if std::env::var_os("CAPTURE_PROBE").is_none() {
            run_capture_probe();
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let folder =
            std::path::PathBuf::from(std::env::var_os("XDG_PICTURES_DIR").unwrap()).join("Screenshots");

        let _missing = driver
            .execute(
                &mut compositor,
                Request::Say {
                    text: "screenshot window".into(),
                },
            )
            .unwrap();
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("SCREENSHOT FAILED: no focused window")
        );
        assert!(
            screenshot_files(&folder).is_empty(),
            "missing focus writes no file"
        );

        let _full = driver
            .execute(
                &mut compositor,
                Request::Say {
                    text: "Screenshot.".into(),
                },
            )
            .unwrap();
        let after_full = screenshot_files(&folder);
        assert_eq!(after_full.len(), 1);
        assert_eq!(
            image::open(&after_full[0]).unwrap().to_rgba8().dimensions(),
            (1280, 720)
        );

        verify_window_and_pad_capture(&mut display, &mut compositor, &mut driver, &folder, &after_full);
    }

    fn run_capture_probe() {
        let directory = tempfile::tempdir().unwrap();
        let pictures = directory.path().join("Pictures");
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        let _configured = command
            .args([
                "--exact",
                "tests::voice_and_pad_capture_commands_save_screen_and_window_screenshots",
            ])
            .env("CAPTURE_PROBE", "1")
            .env("HOME", directory.path())
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .env("XDG_DATA_HOME", directory.path().join("data"))
            .env("XDG_DATA_DIRS", directory.path().join("none"))
            .env("XDG_PICTURES_DIR", &pictures)
            .env("TEHUTI_TEST_SIZE", "1280x720");
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "capture probe: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn screenshot_files(folder: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().path())
            .collect();
        files.sort();
        files
    }

    fn verify_window_and_pad_capture(
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        driver: &mut Driver,
        folder: &std::path::Path,
        previous: &[std::path::PathBuf],
    ) {
        let (server, client_stream) = std::os::unix::net::UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, std::sync::Arc::new(tehuti::state::ClientState::default()))
            .unwrap();
        let mut harness = crate::harness::Harness::new();
        let (ready_tx, ready_rx) = harness.channel::<()>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let client = harness.client(move || {
            coloured_window_client(client_stream, &ready_tx, &stop_rx);
        });
        harness.serve_until(display, compositor, || ready_rx.try_recv().is_ok());
        compositor.space.refresh();

        let _window_shot = driver
            .execute(
                compositor,
                Request::Say {
                    text: "screenshot window".into(),
                },
            )
            .unwrap();
        let after_voice = screenshot_files(folder);
        let voice_path = after_voice
            .iter()
            .find(|path| !previous.contains(path))
            .expect("voice window screenshot file");
        assert_window_shot(voice_path);

        verify_pad_capture_choice(compositor, driver, folder, &after_voice);

        stop_tx.send(()).unwrap();
        harness.serve_until_ended(display, compositor, &[&client]);
        client.join();
    }

    fn open_capture_choice(compositor: &mut Compositor, driver: &mut Driver) {
        let _hold = driver
            .execute(
                compositor,
                Request::Hold {
                    button: ButtonName::Guide,
                },
            )
            .unwrap();
        let _press = driver
            .execute(
                compositor,
                Request::Press {
                    button: ButtonName::RightBumper,
                },
            )
            .unwrap();
        let _release = driver
            .execute(
                compositor,
                Request::Release {
                    button: ButtonName::Guide,
                },
            )
            .unwrap();
    }

    fn stick_down(compositor: &mut Compositor, driver: &mut Driver) {
        let _down = driver
            .execute(
                compositor,
                Request::Stick {
                    left: (0, 800),
                    right: (0, 0),
                },
            )
            .unwrap();
        let _rest = driver
            .execute(
                compositor,
                Request::Stick {
                    left: (0, 0),
                    right: (0, 0),
                },
            )
            .unwrap();
    }

    fn verify_pad_capture_choice(
        compositor: &mut Compositor,
        driver: &mut Driver,
        folder: &std::path::Path,
        previous: &[std::path::PathBuf],
    ) {
        open_capture_choice(compositor, driver);
        let scene = driver.execute(compositor, Request::Scene).unwrap();
        let rows: Vec<_> = scene["launcher"]["rows"]
            .as_array()
            .expect("capture choice rows")
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(rows, vec!["Screenshot", "Screenshot window", "Start recording"]);
        assert_eq!(scene["launcher"]["selection"], 0);
        if let Some(shot_dir) = std::env::var_os("SHOT_DIR") {
            let shot_path = std::path::PathBuf::from(shot_dir).join("capture-choice.png");
            let _saved = driver
                .execute(compositor, Request::Shot { path: shot_path })
                .unwrap();
        }
        stick_down(compositor, driver);
        assert_eq!(
            driver.execute(compositor, Request::Scene).unwrap()["launcher"]["selection"],
            1
        );
        let _confirm = driver
            .execute(
                compositor,
                Request::Press {
                    button: ButtonName::South,
                },
            )
            .unwrap();
        assert!(
            driver.execute(compositor, Request::Scene).unwrap()["launcher"].is_null(),
            "confirm closes capture choice"
        );
        let after_pad = screenshot_files(folder);
        let pad_path = after_pad
            .iter()
            .find(|path| !previous.contains(path))
            .expect("pad window screenshot file");
        assert_window_shot(pad_path);
        verify_pad_capture_cancel(compositor, driver, folder, after_pad.len());
    }

    fn verify_pad_capture_cancel(
        compositor: &mut Compositor,
        driver: &mut Driver,
        folder: &std::path::Path,
        expected_count: usize,
    ) {
        open_capture_choice(compositor, driver);
        let _cancel = driver
            .execute(
                compositor,
                Request::Press {
                    button: ButtonName::East,
                },
            )
            .unwrap();
        assert!(
            driver.execute(compositor, Request::Scene).unwrap()["launcher"].is_null(),
            "back cancels capture choice"
        );
        assert_eq!(screenshot_files(folder).len(), expected_count);
    }

    fn assert_window_shot(path: &std::path::Path) {
        let shot = image::open(path).unwrap().to_rgba8();
        let (width, height) = shot.dimensions();
        assert!(
            width > 0 && width < 1280,
            "window shot width {width} must be cropped"
        );
        assert!(
            height > 0 && height < 720,
            "window shot height {height} must be cropped"
        );
        let mid_x = width.checked_div(2).unwrap();
        let mid_y = height.checked_div(2).unwrap();
        assert_eq!(*shot.get_pixel(mid_x, mid_y), image::Rgba([220, 40, 140, 255]));
    }

    #[derive(Default)]
    struct WindowClient {
        acked: bool,
        size: (i32, i32),
    }

    impl
        wayland_client::Dispatch<
            wayland_client::protocol::wl_registry::WlRegistry,
            wayland_client::globals::GlobalListContents,
        > for WindowClient
    {
        fn event(
            _: &mut Self,
            _: &wayland_client::protocol::wl_registry::WlRegistry,
            _: wayland_client::protocol::wl_registry::Event,
            _: &wayland_client::globals::GlobalListContents,
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
        }
    }

    impl wayland_client::Dispatch<wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase, ()>
        for WindowClient
    {
        fn event(
            _: &mut Self,
            wm_base: &wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase,
            event: wayland_protocols::xdg::shell::client::xdg_wm_base::Event,
            (): &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
            if let wayland_protocols::xdg::shell::client::xdg_wm_base::Event::Ping { serial } = event {
                wm_base.pong(serial);
            }
        }
    }

    impl wayland_client::Dispatch<wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface, ()>
        for WindowClient
    {
        fn event(
            state: &mut Self,
            surface: &wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface,
            event: wayland_protocols::xdg::shell::client::xdg_surface::Event,
            (): &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
            if let wayland_protocols::xdg::shell::client::xdg_surface::Event::Configure { serial } = event {
                surface.ack_configure(serial);
                state.acked = true;
            }
        }
    }

    impl wayland_client::Dispatch<wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel, ()>
        for WindowClient
    {
        fn event(
            state: &mut Self,
            _: &wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
            event: wayland_protocols::xdg::shell::client::xdg_toplevel::Event,
            (): &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
            if let wayland_protocols::xdg::shell::client::xdg_toplevel::Event::Configure {
                width,
                height,
                ..
            } = event
            {
                state.size = (
                    if width > 0 { width } else { 400 },
                    if height > 0 { height } else { 300 },
                );
            }
        }
    }

    wayland_client::delegate_noop!(WindowClient: ignore wayland_client::protocol::wl_surface::WlSurface);
    wayland_client::delegate_noop!(WindowClient: ignore wayland_client::protocol::wl_compositor::WlCompositor);
    wayland_client::delegate_noop!(WindowClient: ignore wayland_client::protocol::wl_output::WlOutput);
    wayland_client::delegate_noop!(WindowClient: ignore wayland_client::protocol::wl_shm::WlShm);
    wayland_client::delegate_noop!(WindowClient: ignore wayland_client::protocol::wl_shm_pool::WlShmPool);
    wayland_client::delegate_noop!(WindowClient: ignore wayland_client::protocol::wl_buffer::WlBuffer);

    fn coloured_window_client(
        stream: std::os::unix::net::UnixStream,
        ready: &crate::harness::Notifier<()>,
        stop: &std::sync::mpsc::Receiver<()>,
    ) {
        use std::{io::Write as _, os::fd::AsFd as _};
        use wayland_client::{
            Connection,
            globals::registry_queue_init,
            protocol::{wl_compositor::WlCompositor, wl_shm},
        };
        use wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;

        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<WindowClient>(&connection).unwrap();
        let handle = queue.handle();
        let wl_compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let surface = wl_compositor.create_surface(&handle, ());
        let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg.get_toplevel(&handle, ());
        toplevel.set_app_id("org.example.CaptureTarget".into());
        surface.commit();
        let mut client = WindowClient {
            acked: false,
            size: (400, 300),
        };
        while !client.acked {
            let _dispatched = queue.roundtrip(&mut client).unwrap();
        }
        let width = client.size.0.min(400);
        let height = client.size.1.min(300);
        let stride = width.saturating_mul(4);
        let bytes = stride.saturating_mul(height);
        let mut file = tempfile::tempfile().unwrap();
        let pixel = [140_u8, 40, 220, 255];
        let count = usize::try_from(width.saturating_mul(height)).unwrap();
        let buffer_bytes: Vec<u8> = pixel.into_iter().cycle().take(count.saturating_mul(4)).collect();
        file.write_all(&buffer_bytes).unwrap();
        let pool = shm.create_pool(file.as_fd(), bytes, &handle, ());
        let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &handle, ());
        xdg.set_window_geometry(0, 0, width, height);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        let _dispatched = queue.roundtrip(&mut client).unwrap();
        ready.send(()).unwrap();
        let _stopped = stop.recv();
    }
}
