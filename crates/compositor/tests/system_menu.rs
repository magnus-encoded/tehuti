//! The system menu end to end through the compositor, with a fake executor so nothing real runs.

#[cfg(test)]
mod system_menu {
    use core::time::Duration;
    use std::sync::{Arc, Mutex};

    use smithay::reexports::wayland_server::Display;
    use tehuti::{state::Compositor, system::Executor};
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    type Calls = Arc<Mutex<Vec<(String, Vec<String>)>>>;

    #[derive(Debug, Default)]
    struct Fake {
        calls: Calls,
        locker: Option<String>,
        fail: bool,
    }

    impl Executor for Fake {
        fn spawn(&mut self, program: &str, args: &[&str]) -> std::io::Result<()> {
            self.calls
                .lock()
                .map_err(|_| std::io::Error::other("poisoned"))?
                .push((program.into(), args.iter().map(|arg| (*arg).into()).collect()));
            if self.fail {
                return Err(std::io::Error::other("fake failure"));
            }
            Ok(())
        }

        fn locker(&self) -> Option<String> {
            self.locker.clone()
        }
    }

    fn setup(locker: Option<&str>, fail: bool) -> (Display<Compositor>, Compositor, Calls) {
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let calls = Calls::default();
        compositor.set_system_executor(Box::new(Fake {
            calls: calls.clone(),
            locker: locker.map(str::to_owned),
            fail,
        }));
        compositor.set_home(None);
        (display, compositor, calls)
    }

    fn press(compositor: &mut Compositor, name: ButtonName) {
        compositor.handle(&Event::Button {
            name,
            state: ButtonState::Pressed,
        });
    }

    fn open(compositor: &mut Compositor) {
        press(compositor, ButtonName::Guide);
        compositor.handle(&Event::Tick {
            elapsed: Duration::from_secs(3),
        });
    }

    fn choose(compositor: &mut Compositor, downs: usize) {
        open(compositor);
        for _ in 0..downs {
            press(compositor, ButtonName::DpadDown);
        }
        press(compositor, ButtonName::South);
    }

    #[test]
    fn holding_guide_opens_the_menu_instead_of_quitting() {
        let (_display, mut compositor, calls) = setup(None, false);
        open(&mut compositor);
        assert!(!compositor.quit_requested());
        let text = compositor.visible_notice_text().unwrap();
        assert!(text.starts_with("SYSTEM MENU\n> CLOSE APP"));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn cancel_and_b_and_guide_close_without_effect() {
        for close in [None, Some(ButtonName::East), Some(ButtonName::Guide)] {
            let (_display, mut compositor, calls) = setup(None, false);
            match close {
                None => choose(&mut compositor, 7),
                Some(name) => {
                    open(&mut compositor);
                    press(&mut compositor, name);
                }
            }
            assert!(
                compositor
                    .visible_notice_text()
                    .is_none_or(|text| !text.starts_with("SYSTEM"))
            );
            assert!(!compositor.quit_requested());
            assert!(calls.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn reboot_and_power_off_spawn_systemctl_without_sudo() {
        for (downs, verb) in [(5, "reboot"), (6, "poweroff")] {
            let (_display, mut compositor, calls) = setup(None, false);
            choose(&mut compositor, downs);
            assert_eq!(
                *calls.lock().unwrap(),
                vec![("systemctl".to_owned(), vec![verb.to_owned()])]
            );
            assert!(!compositor.quit_requested());
        }
    }

    #[test]
    fn a_failed_spawn_shows_a_notice() {
        let (_display, mut compositor, _calls) = setup(None, true);
        choose(&mut compositor, 5);
        assert_eq!(compositor.visible_notice_text().as_deref(), Some("REBOOT FAILED"));
    }

    #[test]
    fn log_out_and_restart_quit_with_their_exit_kind() {
        let (_display, mut compositor, calls) = setup(None, false);
        choose(&mut compositor, 3);
        assert!(compositor.quit_requested() && compositor.log_out_requested());
        let (_second_display, mut restarted, _) = setup(None, false);
        choose(&mut restarted, 4);
        assert!(restarted.quit_requested() && !restarted.log_out_requested());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn lock_without_a_locker_says_not_available() {
        let (_display, mut compositor, calls) = setup(None, false);
        choose(&mut compositor, 2);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("LOCK NOT AVAILABLE")
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn lock_with_a_locker_starts_it() {
        let (_display, mut compositor, calls) = setup(Some("mylocker"), false);
        choose(&mut compositor, 2);
        assert_eq!(*calls.lock().unwrap(), vec![("mylocker".to_owned(), vec![])]);
    }

    #[test]
    fn screenshot_requests_a_file_under_pictures_screenshots() {
        let (_display, mut compositor, calls) = setup(None, false);
        let home = tempfile::tempdir().unwrap();
        compositor.set_home(Some(home.path().to_owned()));
        choose(&mut compositor, 1);
        let path = compositor.take_screenshot_request().unwrap();
        assert_eq!(path.parent().unwrap(), home.path().join("Pictures/Screenshots"));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("tehuti-") && name.ends_with(".png".to_lowercase().as_str()));
        assert!(compositor.take_screenshot_request().is_none());
        compositor.screenshot_saved(&Ok(()));
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("SCREENSHOT SAVED")
        );
        compositor.screenshot_saved(&Err(std::io::Error::other("disk")));
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("SCREENSHOT FAILED")
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn screenshot_without_a_home_fails_visibly() {
        let (_display, mut compositor, _calls) = setup(None, false);
        choose(&mut compositor, 1);
        assert!(compositor.take_screenshot_request().is_none());
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("SCREENSHOT FAILED")
        );
    }

    #[test]
    fn timestamps_are_utc_civil_time() {
        assert_eq!(tehuti::system::timestamp(0), "19700101-000000");
        assert_eq!(tehuti::system::timestamp(1_700_000_000), "20231114-221320");
        assert_eq!(tehuti::system::timestamp(1_709_164_800), "20240229-000000");
    }

    #[test]
    fn saved_png_round_trips_and_creates_directories() {
        let home = tempfile::tempdir().unwrap();
        let path = tehuti::system::screenshot_path(home.path(), 1_700_000_000);
        assert!(path.ends_with("Pictures/Screenshots/tehuti-20231114-221320.png"));
        let pixels: Vec<u8> = (0..2 * 3 * 4).map(|value| u8::try_from(value).unwrap()).collect();
        tehuti::system::save_png(&path, 2, 3, &pixels).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!((info.width, info.height), (2, 3));
        assert_eq!(&buffer[..info.buffer_size()], &pixels[..]);
        assert!(tehuti::system::save_png(&path, 2, 3, &pixels[1..]).is_err());
    }

    #[test]
    fn captured_frames_become_opaque_and_upright() {
        let two_rows = vec![1, 2, 3, 0, 4, 5, 6, 0];
        assert_eq!(
            tehuti::system::normalize(two_rows.clone(), 1, false),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
        assert_eq!(
            tehuti::system::normalize(two_rows, 1, true),
            vec![4, 5, 6, 255, 1, 2, 3, 255]
        );
    }
}
