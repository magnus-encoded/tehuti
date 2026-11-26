//! End-to-end compositor tests for update badge, voice commands, Settings row and rollback recovery.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
    };

    use smithay::{
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        reexports::wayland_server::Display,
        utils::Transform,
    };
    use tehuti::{
        state::Compositor,
        update::{Config, DEFAULT_CHECK_INTERVAL, Manager},
    };
    use tehuti_core::{dispatch::Category, overlay::Overlay};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    struct ReleaseFixture {
        _dir: tempfile::TempDir,
        lib_dir: PathBuf,
        state_dir: PathBuf,
        log_path: PathBuf,
        script: PathBuf,
    }

    impl ReleaseFixture {
        fn new(with_previous: bool) -> Result<Self, Box<dyn core::error::Error>> {
            let dir = tempfile::tempdir()?;
            let lib_dir = dir.path().join("lib");
            let state_dir = dir.path().join("state");
            fs::create_dir_all(&lib_dir)?;
            fs::create_dir_all(&state_dir)?;
            write_executable(&lib_dir.join("tehuti"), "#!/bin/sh\nexit 0\n")?;
            fs::write(lib_dir.join("tehuti.version"), "v0.148.2\n")?;
            if with_previous {
                write_executable(&lib_dir.join("tehuti.prev"), "#!/bin/sh\nexit 0\n")?;
                fs::write(lib_dir.join("tehuti.prev.version"), "v0.148.1\n")?;
            }
            let log_path = dir.path().join("calls.log");
            let script = stub_update_script(dir.path(), &log_path)?;
            Ok(Self {
                _dir: dir,
                lib_dir,
                state_dir,
                log_path,
                script,
            })
        }

        fn manager(&self) -> Manager {
            Manager::new(Config {
                script: Some(self.script.clone()),
                lib_dir: self.lib_dir.clone(),
                state_dir: self.state_dir.clone(),
                channel: None,
                check_interval: DEFAULT_CHECK_INTERVAL,
            })
        }
    }

    fn write_executable(path: &Path, body: &str) -> Result<(), Box<dyn core::error::Error>> {
        fs::write(path, body)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    fn stub_update_script(directory: &Path, log_path: &Path) -> Result<PathBuf, Box<dyn core::error::Error>> {
        let script = directory.join("tehuti-update");
        let body = format!(
            "#!/bin/sh\nset -eu\nlib=${{TEHUTI_LIB:?}}\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"${{1:-}}\" in\n  --check)\n    installed=$(cat \"$lib/tehuti.version\" 2>/dev/null || echo unknown)\n    fallback=$(cat \"$lib/tehuti.prev.version\" 2>/dev/null || echo unknown)\n    echo \"channel latest: installed $installed, fallback $fallback, newest v0.149.0 (https://example.test/tehuti.tar.gz)\"\n    ;;\n  --rollback)\n    bad=$(cat \"$lib/tehuti.version\")\n    good=$(cat \"$lib/tehuti.prev.version\")\n    mv \"$lib/tehuti\" \"$lib/tehuti.tmp\"\n    mv \"$lib/tehuti.prev\" \"$lib/tehuti\"\n    mv \"$lib/tehuti.tmp\" \"$lib/tehuti.prev\"\n    printf '%s\\n' \"$good\" > \"$lib/tehuti.version\"\n    printf '%s\\n' \"$bad\" > \"$lib/tehuti.prev.version\"\n    printf '%s\\n' \"$bad\" > \"$lib/tehuti.skip\"\n    ;;\n  '')\n    old=$(cat \"$lib/tehuti.version\")\n    mv \"$lib/tehuti\" \"$lib/tehuti.prev\"\n    printf '%s\\n' \"$old\" > \"$lib/tehuti.prev.version\"\n    printf '#!/bin/sh\\nexit 0\\n' > \"$lib/tehuti\"\n    chmod 755 \"$lib/tehuti\"\n    printf 'v0.149.0\\n' > \"$lib/tehuti.version\"\n    ;;\nesac\n",
            log_path.display()
        );
        write_executable(&script, &body)?;
        Ok(script)
    }

    fn tap(compositor: &mut Compositor, name: ButtonName) {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            compositor.handle(&Event::Button { name, state });
        }
    }

    fn poll_until(
        compositor: &mut Compositor,
        condition: impl Fn(&Compositor) -> bool,
    ) -> Result<(), Box<dyn core::error::Error>> {
        for _ in 0..200 {
            compositor.handle(&Event::Tick {
                elapsed: Duration::from_millis(10),
            });
            if condition(compositor) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("timed out waiting for background update worker".into())
    }

    fn test_output() -> Output {
        let output = Output::new(
            "update-test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1280, 720).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        output
    }

    fn settings_pixels(compositor: &Compositor) -> Result<Vec<u8>, Box<dyn core::error::Error>> {
        let mut artwork = compositor
            .settings_buffer((1280, 720))
            .ok_or("settings artwork missing")?;
        let mut copy = Vec::new();
        artwork.render().draw(|pixels| {
            copy = pixels.to_vec();
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(copy)
    }

    #[test]
    fn update_check_shows_bar_chip_and_voice_update_applies_and_clears_chip()
    -> Result<(), Box<dyn core::error::Error>> {
        let fixture = ReleaseFixture::new(false)?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.set_update_manager(fixture.manager());
        compositor.check_for_updates();

        poll_until(&mut compositor, |state| {
            state
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Update")
        })?;

        compositor.handle(&Event::Speech {
            text: Text::new("Update".into())?,
        });
        let Some((layer, prompt)) = compositor.visible_notice() else {
            return Err("expected update confirmation notice".into());
        };
        assert_eq!(layer, Overlay::UpdateConfirmation);
        assert!(
            prompt.contains("UPDATE TO V0.149.0?"),
            "confirmation prompt names target version: {prompt}"
        );
        tap(&mut compositor, ButtonName::South);

        poll_until(&mut compositor, |state| {
            !state
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "Update")
                && state.settings().previous_version.as_deref() == Some("v0.148.2")
        })?;

        let notice = compositor.visible_notice_text().unwrap_or_default();
        assert!(
            notice.contains("UPDATED TO V0.149.0"),
            "post-update notice names installed release: {notice}"
        );
        assert_eq!(
            fs::read_to_string(fixture.lib_dir.join("tehuti.version"))?.trim(),
            "v0.149.0"
        );
        Ok(())
    }

    fn bar_pixels(
        bar_buffer: &mut tehuti::bar::Buffer,
        output: &Output,
        compositor: &Compositor,
    ) -> Result<Vec<u8>, Box<dyn core::error::Error>> {
        let mut rendered = bar_buffer
            .update(output, &compositor.voice_bar(), "12:00")
            .ok_or("bar buffer missing")?
            .0
            .clone();
        let mut copy = Vec::new();
        rendered.render().draw(|pixels| {
            copy = pixels.to_vec();
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(copy)
    }

    #[test]
    fn focusing_the_update_bar_chip_redraws_the_bar_buffer() -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.set_release_info(tehuti_core::update::ReleaseInfo {
            installed: Some("v0.148.2".into()),
            previous: None,
            available: Some("v0.149.0".into()),
        });
        let output = test_output();
        let mut bar_buffer = tehuti::bar::Buffer::default();
        let unfocused = bar_pixels(&mut bar_buffer, &output, &compositor)?;
        tap(&mut compositor, ButtonName::Guide);
        tap(&mut compositor, ButtonName::DpadDown);
        assert!(
            compositor.voice_bar().update_focused(),
            "D-pad focuses the Update chip on the bar"
        );
        let focused = bar_pixels(&mut bar_buffer, &output, &compositor)?;
        assert_ne!(
            unfocused, focused,
            "focused update chip changes the rasterised bar pixels"
        );
        tap(&mut compositor, ButtonName::East);
        assert!(
            !compositor.voice_bar().update_focused(),
            "Back clears focus on the Update chip"
        );
        Ok(())
    }

    #[test]
    fn settings_previous_version_row_renders_and_runs_rollback_script()
    -> Result<(), Box<dyn core::error::Error>> {
        let fixture = ReleaseFixture::new(true)?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.set_update_manager(fixture.manager());

        compositor.handle(&Event::Speech {
            text: Text::new("Pop the hood.".into())?,
        });
        for _ in 0..3 {
            tap(&mut compositor, ButtonName::DpadDown);
        }
        assert_eq!(compositor.settings().category, Category::PreviousVersion);
        let before_pixels = settings_pixels(&compositor)?;

        tap(&mut compositor, ButtonName::South);
        assert!(
            compositor.settings().pending_rollback,
            "first Confirm highlights rollback confirmation"
        );
        let confirming_pixels = settings_pixels(&compositor)?;
        assert_ne!(
            before_pixels, confirming_pixels,
            "confirmation state changes the rendered Settings page"
        );

        tap(&mut compositor, ButtonName::South);
        poll_until(&mut compositor, |state| {
            state.settings().previous_version.as_deref() == Some("v0.148.2")
        })?;

        assert_eq!(
            fs::read_to_string(fixture.lib_dir.join("tehuti.version"))?.trim(),
            "v0.148.1"
        );
        assert_eq!(
            fs::read_to_string(fixture.lib_dir.join("tehuti.skip"))?.trim(),
            "v0.148.2"
        );
        assert!(
            fs::read_to_string(&fixture.log_path)?.contains("--rollback"),
            "rollback reused tehuti-update --rollback"
        );
        Ok(())
    }

    #[test]
    fn repeated_failed_starts_auto_offer_rollback_and_confirm_executes_script()
    -> Result<(), Box<dyn core::error::Error>> {
        let fixture = ReleaseFixture::new(true)?;
        fs::write(fixture.state_dir.join("rollback-offer"), "1000\n")?;

        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.enable_invitation();
        compositor.set_update_manager(fixture.manager());

        let Some((layer, prompt)) = compositor.visible_notice() else {
            return Err("expected auto-offered rollback confirmation".into());
        };
        assert_eq!(layer, Overlay::UpdateConfirmation);
        assert!(
            prompt.contains("REPEATED FAILED STARTS") && prompt.contains("PREVIOUS VERSION V0.148.1?"),
            "auto-offer prompt names repeated failures and previous version: {prompt}"
        );

        tap(&mut compositor, ButtonName::South);
        poll_until(&mut compositor, |state| {
            state.settings().previous_version.as_deref() == Some("v0.148.2")
        })?;
        assert_eq!(
            fs::read_to_string(fixture.lib_dir.join("tehuti.version"))?.trim(),
            "v0.148.1"
        );
        Ok(())
    }
}
