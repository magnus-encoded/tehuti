#[cfg(test)]
mod tests {
    use serde_json::Value;
    use std::{
        path::Path,
        process::{Command, Output},
    };

    fn drive(socket: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tehuti-drive"))
            .arg("--socket")
            .arg(socket)
            .args(arguments)
            .output()
            .unwrap()
    }

    fn malformed_clients_do_not_block_the_session(socket: &Path) {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixStream,
        };
        let _idle = UnixStream::connect(socket).unwrap();
        let mut malformed = UnixStream::connect(socket).unwrap();
        malformed
            .set_read_timeout(Some(core::time::Duration::from_secs(2)))
            .unwrap();
        malformed.write_all(b"{\"action\":\"unknown\"}\n").unwrap();
        let mut reply = String::new();
        let _count = malformed.read_to_string(&mut reply).unwrap();
        assert!(
            serde_json::from_str::<Value>(&reply)
                .unwrap()
                .get("error")
                .is_some()
        );
        assert!(drive(socket, &["scene", "--json"]).status.success());
        assert!(!drive(socket, &["stick", "1001", "0", "0", "0"]).status.success());
        assert!(!drive(socket, &["key", "unknown"]).status.success());
    }

    fn wait_for_scene(child: &mut std::process::Child, socket: &Path, log_path: &Path) {
        let deadline = std::time::Instant::now()
            .checked_add(core::time::Duration::from_secs(10))
            .unwrap();
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(log_path).unwrap()
            );
            if drive(socket, &["scene", "--json"]).status.success() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "session did not start");
            std::thread::sleep(core::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn typed_speech_startup_recovers_stale_fifos_and_preserves_other_files() {
        use std::os::unix::fs::FileTypeExt as _;
        for stale_fifo in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let config = root.path().join("config");
            std::fs::create_dir(&config).unwrap();
            let path = root.path().join(tehuti::say::FIFO_NAME);
            if stale_fifo {
                rustix::fs::mkfifoat(rustix::fs::CWD, &path, rustix::fs::Mode::RUSR).unwrap();
            } else {
                std::fs::write(&path, b"keep").unwrap();
            }
            let log_path = root.path().join("session.log");
            let log = std::fs::File::create(&log_path).unwrap();
            let mut child = Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .arg("--test-driver")
                .arg(&config)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", root.path())
                .env("XDG_RUNTIME_DIR", root.path())
                .env("XDG_CONFIG_HOME", &config)
                .env("XDG_DATA_HOME", root.path().join("data"))
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env("TEHUTI_SAY", "1")
                .env("TEHUTI_TEST_SIZE", "320x240")
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap();
            let socket = root.path().join("tehuti-drive.sock");
            let checked = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
                wait_for_scene(&mut child, &socket, &log_path);
                if stale_fifo {
                    assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_fifo());
                } else {
                    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
                    assert!(
                        std::fs::read_to_string(&log_path)
                            .unwrap()
                            .contains("typed speech FIFO unavailable")
                    );
                }
            }));
            let _stopped = drive(&socket, &["stop"]);
            let _killed = child.kill();
            let _waited = child.wait();
            if let Err(panic) = checked {
                std::panic::resume_unwind(panic);
            }
        }
    }

    #[test]
    fn headless_session_can_be_inspected_advanced_captured_and_stopped() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        std::fs::create_dir(&config).unwrap();
        let socket = root.path().join("runtime/tehuti-drive.sock");
        let start = drive(
            &socket,
            &[
                "start",
                config.to_str().unwrap(),
                "--headless",
                "--size",
                "320x240",
            ],
        );
        assert!(
            start.status.success(),
            "{}",
            String::from_utf8_lossy(&start.stderr)
        );
        let result = || {
            malformed_clients_do_not_block_the_session(&socket);
            let scene = drive(&socket, &["scene", "--json"]);
            assert!(scene.status.success());
            let scene: Value = serde_json::from_slice(&scene.stdout).unwrap();
            assert_eq!(scene["windows"], serde_json::json!([]));
            assert!(drive(&socket, &["clock", "advance", "5000"]).status.success());
            assert!(
                drive(&socket, &["wait-for", "/windows", "[]", "100"])
                    .status
                    .success()
            );
            assert!(
                !drive(&socket, &["wait-for", "/windows", "[1]", "0"])
                    .status
                    .success()
            );
            let image = root.path().join("shot.png");
            let capture = drive(&socket, &["shot", image.to_str().unwrap()]);
            assert!(
                capture.status.success(),
                "{}",
                String::from_utf8_lossy(&capture.stderr)
            );
            assert_eq!(image::image_dimensions(image).unwrap(), (320, 240));
        };
        let checked = std::panic::catch_unwind(result);
        assert!(drive(&socket, &["stop"]).status.success());
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    }
}
