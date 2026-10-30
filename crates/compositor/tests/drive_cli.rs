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
