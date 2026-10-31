#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        driver::{Driver, Request},
        state::Compositor,
    };
    use tehuti_protocol::ButtonName;

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
        let index = tehuti_core::menu::ITEMS
            .iter()
            .position(|item| item.key() == "screenshot")
            .expect("Screenshot menu item");
        for _ in 0..index {
            let _moved = driver
                .execute(
                    compositor,
                    Request::Press {
                        button: ButtonName::DpadDown,
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
}
