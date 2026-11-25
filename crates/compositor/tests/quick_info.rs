#[cfg(test)]
mod quick_info {
    use std::path::Path;

    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        config_check::{Severity, check},
        notice::notice_buffer,
        quick_info::{Source, read_battery},
        state::Compositor,
    };
    use tehuti_core::quick_info::{Battery, BatteryStatus, LIFETIME};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text, parse_context};

    fn write_file(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn notice_pixels(text: &str) -> Vec<u8> {
        let mut buffer = notice_buffer(text);
        let mut result = Vec::new();
        buffer
            .render()
            .draw::<_, core::convert::Infallible>(|pixels| {
                result = pixels.to_vec();
                Ok(Vec::new())
            })
            .unwrap();
        result
    }

    fn compositor_with(power_supply: &Path) -> (Display<Compositor>, Compositor) {
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let global = parse_context("app_id = '*'\n[buttons]\nwest = { info = 'cycle' }\n").unwrap();
        compositor.set_contexts(vec![global]);
        compositor.set_quick_info_source(Source::with_paths_and_clock(power_supply.to_path_buf(), || {
            ("14:35".into(), "Wed 2026-11-25".into())
        }));
        (display, compositor)
    }

    #[test]
    fn voice_phrases_render_time_and_battery_notices() {
        let power = tempfile::tempdir().unwrap();
        write_file(power.path(), "BAT0/type", "Battery\n");
        write_file(power.path(), "BAT0/capacity", "82\n");
        write_file(power.path(), "BAT0/status", "Charging\n");

        let (_display, mut compositor) = compositor_with(power.path());
        compositor.handle(&Event::Speech {
            text: Text::new("What time is it?".into()).unwrap(),
        });
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("what time is it: 14:35")
        );

        compositor.handle(&Event::Speech {
            text: Text::new("Battery.".into()).unwrap(),
        });
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("battery: 82% charging")
        );
    }

    #[test]
    fn missing_battery_renders_clear_no_battery_notice() {
        let power = tempfile::tempdir().unwrap();
        write_file(power.path(), "AC/type", "Mains\n");
        write_file(power.path(), "hidpp_battery_0/type", "Battery\n");
        write_file(power.path(), "hidpp_battery_0/scope", "Device\n");
        write_file(power.path(), "hidpp_battery_0/capacity", "95\n");

        let (_display, mut compositor) = compositor_with(power.path());
        compositor.handle(&Event::Speech {
            text: Text::new("battery".into()).unwrap(),
        });
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("battery: no battery")
        );
    }

    fn press_button(compositor: &mut Compositor, name: ButtonName) {
        compositor.handle(&Event::Button {
            name,
            state: ButtonState::Pressed,
        });
        compositor.handle(&Event::Button {
            name,
            state: ButtonState::Released,
        });
    }

    #[test]
    fn pad_cycle_steps_notices_and_b_dismisses() {
        let power = tempfile::tempdir().unwrap();
        write_file(power.path(), "BAT0/type", "Battery\n");
        write_file(power.path(), "BAT0/capacity", "64\n");
        write_file(power.path(), "BAT0/status", "Discharging\n");

        let (_display, mut compositor) = compositor_with(power.path());

        press_button(&mut compositor, ButtonName::West);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("what time is it: 14:35")
        );

        press_button(&mut compositor, ButtonName::West);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("date: Wed 2026-11-25")
        );

        press_button(&mut compositor, ButtonName::West);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("battery: 64% discharging")
        );

        press_button(&mut compositor, ButtonName::West);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("what time is it: 14:35")
        );

        press_button(&mut compositor, ButtonName::East);
        assert_eq!(compositor.visible_notice_text(), None);

        press_button(&mut compositor, ButtonName::West);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("what time is it: 14:35")
        );
    }

    #[test]
    fn expiry_of_an_old_info_notice_does_not_clear_a_newer_unrelated_notice() {
        let power = tempfile::tempdir().unwrap();
        let (_display, mut compositor) = compositor_with(power.path());
        compositor.handle(&Event::Speech {
            text: Text::new("what time is it".into()).unwrap(),
        });
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("what time is it: 14:35")
        );

        compositor.handle(&Event::Speech {
            text: Text::new("completely unmapped phrase".into()).unwrap(),
        });
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("NO SPEECH MAPPING")
        );

        compositor.handle(&Event::Tick { elapsed: LIFETIME });
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("NO SPEECH MAPPING")
        );
    }

    #[test]
    fn read_battery_handles_energy_charge_and_multiple_system_batteries() {
        let energy = tempfile::tempdir().unwrap();
        write_file(energy.path(), "BAT0/type", "Battery\n");
        write_file(energy.path(), "BAT0/energy_now", "2500\n");
        write_file(energy.path(), "BAT0/energy_full", "5000\n");
        write_file(energy.path(), "BAT0/status", "Full\n");
        assert_eq!(
            read_battery(energy.path()),
            Some(Battery::new(50, BatteryStatus::Full))
        );

        let dual = tempfile::tempdir().unwrap();
        write_file(dual.path(), "BAT0/type", "Battery\n");
        write_file(dual.path(), "BAT0/charge_now", "800\n");
        write_file(dual.path(), "BAT0/charge_full", "1000\n");
        write_file(dual.path(), "BAT0/status", "Full\n");
        write_file(dual.path(), "BAT1/type", "Battery\n");
        write_file(dual.path(), "BAT1/capacity", "40\n");
        write_file(dual.path(), "BAT1/status", "Charging\n");
        assert_eq!(
            read_battery(dual.path()),
            Some(Battery::new(60, BatteryStatus::Charging))
        );

        let missing = energy.path().join("does-not-exist");
        assert_eq!(read_battery(&missing), None);
    }

    #[test]
    fn percent_glyph_renders_visible_pixels_in_notice_buffer() {
        assert_ne!(
            notice_pixels("battery: 82%"),
            notice_pixels("battery: 82 "),
            "percent sign must render a visible glyph"
        );
    }

    #[test]
    fn config_check_warns_about_reserved_quick_info_speech_phrases() {
        let directory = tempfile::tempdir().unwrap();
        write_file(
            directory.path(),
            "global/context.toml",
            "app_id = '*'\n[speech]\n'What time is it?' = { key = 'enter' }\n\
             'battery' = { key = 'space' }\n",
        );
        let report = check(directory.path());
        assert!(!report.has_errors(), "{report}");
        assert_eq!(report.findings.len(), 2, "{report}");
        for finding in &report.findings {
            assert_eq!(finding.severity, Severity::Warning);
            assert!(
                finding.message.contains("reserved info phrase"),
                "unexpected warning: {}",
                finding.message
            );
        }
    }
}
