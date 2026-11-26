//! The pad closes the program holding the display, and leaving the session warns about it.

#[cfg(test)]
mod display_holders {
    use core::time::Duration;
    use smithay::reexports::wayland_server::Display;
    use std::{fs, os::unix::fs::symlink};
    use tehuti::state::Compositor;
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    const DEVICE: &str = "/dev/dri/card-example";

    /// Runs `probe` in a child test process with no user menu file or programs.
    fn isolated(probe: &str) {
        let directory = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", probe])
            .env("TEHUTI_TEST_HOLDER_MENU", "yes")
            .env("TEHUTI_MENU_CONFIG", directory.path().join("missing.toml"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("XDG_DATA_HOME", directory.path())
            .env("XDG_DATA_DIRS", directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    fn holder_in(proc_root: &std::path::Path, pid: u32) {
        let directory = proc_root.join(pid.to_string());
        fs::create_dir_all(directory.join("fd")).unwrap();
        fs::write(directory.join("comm"), "example-player\n").unwrap();
        symlink(DEVICE, directory.join("fd").join("3")).unwrap();
    }

    fn press(target: &mut Compositor, name: ButtonName) {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            target.handle(&Event::Button { name, state });
        }
    }

    fn open_menu(compositor: &mut Compositor) {
        compositor.handle(&Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Pressed,
        });
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_millis(500),
        });
        compositor.handle(&Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Released,
        });
    }

    fn compositor_with_holder(pid: u32) -> (Compositor, tempfile::TempDir, Display<Compositor>) {
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let proc_root = tempfile::tempdir().unwrap();
        holder_in(proc_root.path(), pid);
        compositor.set_display_holders(proc_root.path().into(), DEVICE.into());
        (compositor, proc_root, display)
    }

    #[test]
    fn the_close_row_is_hidden_until_a_holder_exists() {
        isolated("display_holders::hidden_probe");
    }

    #[test]
    fn hidden_probe() {
        if std::env::var_os("TEHUTI_TEST_HOLDER_MENU").is_none() {
            return;
        }
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let empty = tempfile::tempdir().unwrap();
        compositor.set_display_holders(empty.path().into(), DEVICE.into());
        open_menu(&mut compositor);
        assert!(
            !compositor
                .visible_notice_text()
                .is_some_and(|text| text.contains("CLOSE DISPLAY HOLDER"))
        );
        let (mut held, _proc_root, _held_display) = compositor_with_holder(std::process::id() + 1);
        open_menu(&mut held);
        let text = held.visible_notice_text().unwrap();
        assert!(text.contains("CLOSE DISPLAY HOLDER"), "{text}");
    }

    #[test]
    fn choosing_the_close_row_ends_the_holder() {
        isolated("display_holders::close_probe");
    }

    #[test]
    fn close_probe() {
        if std::env::var_os("TEHUTI_TEST_HOLDER_MENU").is_none() {
            return;
        }
        let mut child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        let (mut compositor, _proc_root, _display) = compositor_with_holder(child.id());
        open_menu(&mut compositor);
        let menu_text = compositor.visible_notice_text().unwrap();
        let row = menu_text
            .lines()
            .skip(1)
            .position(|line| line.trim_start_matches(['>', ' ']) == "CLOSE DISPLAY HOLDER")
            .unwrap();
        for _ in 0..row {
            press(&mut compositor, ButtonName::DpadDown);
        }
        press(&mut compositor, ButtonName::South);
        press(&mut compositor, ButtonName::South);
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert!(!compositor.quit_requested());
        let notice = compositor.visible_notice_text().unwrap();
        assert!(notice.contains("example-player"), "{notice}");
    }

    #[test]
    fn leaving_the_session_warns_about_holders_and_still_leaves() {
        isolated("display_holders::warning_probe");
    }

    #[test]
    fn warning_probe() {
        if std::env::var_os("TEHUTI_TEST_HOLDER_MENU").is_none() {
            return;
        }
        let (mut compositor, _proc_root, _display) = compositor_with_holder(std::process::id() + 1);
        open_menu(&mut compositor);
        press(&mut compositor, ButtonName::South);
        press(&mut compositor, ButtonName::South);
        assert!(
            !compositor.quit_requested(),
            "the warning must show before leaving"
        );
        let notice = compositor.visible_notice_text().unwrap();
        assert!(notice.contains("example-player"), "{notice}");
        compositor.advance_time();
        assert!(!compositor.quit_requested());
        std::thread::sleep(Duration::from_millis(2100));
        compositor.advance_time();
        assert!(compositor.quit_requested());
    }
}
