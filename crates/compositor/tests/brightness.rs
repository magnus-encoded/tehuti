#[cfg(test)]
mod tests {
    use core::time::Duration;
    use smithay::reexports::wayland_server::Display;
    use std::{fs, path::Path, sync::mpsc};
    use tehuti::{
        adjuster::raster,
        brightness::{Backend, Error, Reply, Request, parse_ddc_terse, percent_to_raw, raw_to_percent},
        state::Compositor,
    };
    use tehuti_core::{
        adjuster::{
            BRIGHTNESS_MAX, BRIGHTNESS_MIN, BRIGHTNESS_STEP, STEP_REPEAT_DELAY, STEP_REPEAT_INTERVAL,
        },
        bar::Tone,
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    fn collect_replies(backend: &Backend, expected: usize) -> Vec<Reply> {
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap();
        let mut replies = Vec::new();
        while replies.len() < expected && std::time::Instant::now() < deadline {
            replies.extend(backend.poll());
            std::thread::yield_now();
        }
        replies
    }

    fn write_sysfs_device(root: &Path, name: &str, kind: &str, max: u64, actual: u64, brightness: u64) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("type"), format!("{kind}\n")).unwrap();
        fs::write(directory.join("max_brightness"), format!("{max}\n")).unwrap();
        fs::write(directory.join("actual_brightness"), format!("{actual}\n")).unwrap();
        fs::write(directory.join("brightness"), format!("{brightness}\n")).unwrap();
    }

    fn wait_for_file_value(path: &Path, expected: &str) {
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap();
        while std::time::Instant::now() < deadline {
            if fs::read_to_string(path).unwrap_or_default().trim() == expected {
                return;
            }
            std::thread::yield_now();
        }
        assert_eq!(fs::read_to_string(path).unwrap_or_default().trim(), expected);
    }

    #[test]
    fn raw_and_percent_conversions_clamp_and_never_turn_sysfs_off() {
        assert_eq!(raw_to_percent(400, 1000), 40);
        assert_eq!(raw_to_percent(0, 1000), BRIGHTNESS_MIN);
        assert_eq!(raw_to_percent(1500, 1000), BRIGHTNESS_MAX);
        assert_eq!(raw_to_percent(10, 0), BRIGHTNESS_MIN);

        assert_eq!(percent_to_raw(40, 1000), 400);
        assert_eq!(percent_to_raw(0, 1000), 10);
        assert_eq!(
            percent_to_raw(1, 5),
            1,
            "minimum stays lit even on coarse backlights"
        );
        assert_eq!(percent_to_raw(200, 1000), 1000);
        assert_eq!(percent_to_raw(50, 0), 0);

        assert_eq!(parse_ddc_terse("VCP 10 C 40 100\n"), Ok((40, 100)));
        assert_eq!(parse_ddc_terse("noise\nVCP 10 C 128 255\n"), Ok((128, 255)));
        assert!(parse_ddc_terse("VCP 10 C 10 0\n").is_err(), "zero max must fail");
        assert!(parse_ddc_terse("unexpected").is_err(), "invalid reply must fail");
    }

    #[test]
    fn sysfs_backend_prefers_raw_reads_actual_and_clamps_scaled_writes() {
        let sysfs = tempfile::tempdir().unwrap();
        write_sysfs_device(sysfs.path(), "acpi_video0", "firmware", 10, 9, 9);
        write_sysfs_device(sysfs.path(), "intel_backlight", "raw", 1000, 400, 350);

        let backend = Backend::with_sysfs_and_ddc(sysfs.path().to_path_buf(), |_| {
            Err(Error::Failed("ddc must not run when sysfs exists".into()))
        })
        .unwrap();

        backend.request(Request::Read).unwrap();
        backend.request(Request::Set(65)).unwrap();
        backend.request(Request::Set(200)).unwrap();

        let replies = collect_replies(&backend, 3);
        assert_eq!(
            replies,
            [Reply::Read(Ok(40)), Reply::Set(Ok(())), Reply::Set(Ok(()))]
        );
        let path = sysfs.path().join("intel_backlight/brightness");
        assert_eq!(fs::read_to_string(&path).unwrap(), "1000");

        backend.request(Request::Set(0)).unwrap();
        assert_eq!(collect_replies(&backend, 1), [Reply::Set(Ok(()))]);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "10",
            "clamped minimum keeps the panel lit"
        );
    }

    #[test]
    fn ddc_fallback_reads_and_writes_when_sysfs_has_no_backlight() {
        let sysfs = tempfile::tempdir().unwrap();
        let (sent, received) = mpsc::channel();
        let backend = Backend::with_sysfs_and_ddc(sysfs.path().to_path_buf(), move |args| {
            sent.send(args.to_vec())
                .map_err(|error| Error::Failed(error.to_string()))?;
            Ok("VCP 10 C 128 255\n".into())
        })
        .unwrap();

        backend.request(Request::Read).unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap(),
            ["getvcp", "10", "--terse"]
        );
        backend.request(Request::Set(40)).unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap(),
            ["setvcp", "10", "102"]
        );
        assert_eq!(
            collect_replies(&backend, 2),
            [Reply::Read(Ok(50)), Reply::Set(Ok(()))]
        );
    }

    #[test]
    fn missing_backlight_device_shows_clear_notice_without_crashing() {
        let sysfs = tempfile::tempdir().unwrap();
        let backend =
            Backend::with_sysfs_and_ddc(sysfs.path().to_path_buf(), |_| Err(Error::NoDevice)).unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_brightness_backend(backend);

        for phrase in ["brightness", "Brightness 40"] {
            compositor.handle(&Event::Speech {
                text: Text::new(phrase.into()).unwrap(),
            });
            let deadline = std::time::Instant::now()
                .checked_add(Duration::from_secs(2))
                .unwrap();
            while std::time::Instant::now() < deadline {
                compositor.handle(&Event::Tick {
                    elapsed: Duration::ZERO,
                });
                if compositor.visible_notice_text().as_deref() == Some("NO BACKLIGHT DEVICE") {
                    break;
                }
                std::thread::yield_now();
            }
            assert_eq!(
                compositor.visible_notice_text().as_deref(),
                Some("NO BACKLIGHT DEVICE"),
                "{phrase} must show a clear notice without a backlight device"
            );
            assert!(
                compositor.adjuster().is_none(),
                "missing hardware must not open a brightness gauge for {phrase}"
            );
        }
    }

    #[test]
    fn spoken_brightness_destinations_write_hardware_and_show_on_the_bar() {
        let sysfs = tempfile::tempdir().unwrap();
        write_sysfs_device(sysfs.path(), "panel0", "raw", 500, 200, 200);
        let brightness_file = sysfs.path().join("panel0/brightness");
        let backend =
            Backend::with_sysfs_and_ddc(sysfs.path().to_path_buf(), |_| Err(Error::NoDevice)).unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_brightness_backend(backend);

        for (phrase, expected_raw, expected_bar) in [
            ("Brightness 40", "200", "brightness 40"),
            ("brightness maximum", "500", "brightness maximum"),
            ("brightness minimum", "5", "brightness minimum"),
        ] {
            compositor.handle(&Event::Speech {
                text: Text::new(phrase.into()).unwrap(),
            });
            wait_for_file_value(&brightness_file, expected_raw);
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == expected_bar),
                "{phrase} must show {expected_bar} on the bar"
            );
        }
    }

    fn open_brightness_adjuster(compositor: &mut Compositor) {
        compositor.handle(&Event::Speech {
            text: Text::new("brightness".into()).unwrap(),
        });
        let deadline = std::time::Instant::now()
            .checked_add(Duration::from_secs(2))
            .unwrap();
        while compositor.adjuster().is_none() && std::time::Instant::now() < deadline {
            compositor.handle(&Event::Tick {
                elapsed: Duration::ZERO,
            });
            std::thread::yield_now();
        }
    }

    fn bar_has_segment(compositor: &Compositor, text: &str, tone: Option<Tone>) -> bool {
        compositor
            .voice_bar()
            .segments()
            .iter()
            .any(|segment| segment.text == text && tone.is_none_or(|wanted| segment.tone == wanted))
    }

    #[test]
    fn pad_nudges_and_hold_repeat_update_hardware_bar_and_gauge() {
        let sysfs = tempfile::tempdir().unwrap();
        write_sysfs_device(sysfs.path(), "panel0", "raw", 500, 200, 200);
        let brightness_file = sysfs.path().join("panel0/brightness");
        let backend =
            Backend::with_sysfs_and_ddc(sysfs.path().to_path_buf(), |_| Err(Error::NoDevice)).unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_brightness_backend(backend);

        open_brightness_adjuster(&mut compositor);
        let before = raster(compositor.adjuster().unwrap()).unwrap();
        assert_eq!(before.len(), 280 * 224 * 4);
        assert!(
            bar_has_segment(&compositor, "brightness 40", Some(Tone::Live)),
            "bar must show the live spoken brightness unit while adjusting"
        );

        compositor.handle(&Event::Button {
            name: ButtonName::DpadUp,
            state: ButtonState::Pressed,
        });
        assert_eq!(
            compositor.adjuster().unwrap().value(),
            i32::from(40_u8.saturating_add(BRIGHTNESS_STEP))
        );
        wait_for_file_value(&brightness_file, "225");
        assert_ne!(before, raster(compositor.adjuster().unwrap()).unwrap());
        assert!(
            bar_has_segment(&compositor, "brightness 45", Some(Tone::Live)),
            "bar must reflect each pad nudge immediately"
        );

        compositor.handle(&Event::Tick {
            elapsed: STEP_REPEAT_DELAY.saturating_add(STEP_REPEAT_INTERVAL),
        });
        assert_eq!(compositor.adjuster().unwrap().value(), 55);
        wait_for_file_value(&brightness_file, "275");
        compositor.handle(&Event::Button {
            name: ButtonName::DpadUp,
            state: ButtonState::Released,
        });
        compositor.handle(&Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        });
        assert!(compositor.adjuster().is_none(), "South confirms and closes");
        assert!(
            bar_has_segment(&compositor, "brightness 55", None),
            "bar must confirm the final brightness unit"
        );
    }
}
