#[cfg(test)]
mod tests {
    use core::time::Duration;
    use serde_json::{Value, json};
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
        path::Path,
        process::{Command, Output},
        time::Instant,
    };

    #[derive(Debug)]
    struct EventStream(std::process::Child);

    impl Drop for EventStream {
        fn drop(&mut self) {
            let _killed = self.0.kill();
            let _waited = self.0.wait();
        }
    }

    fn await_events(path: &Path, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = std::fs::read_to_string(path).unwrap();
            if text.ends_with('\n') && text.contains(needle) {
                return text;
            }
            assert!(Instant::now() < deadline, "event stream did not produce {needle}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn drive(socket: &Path, words: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tehuti-drive"))
            .arg("--socket")
            .arg(socket)
            .args(words)
            .output()
            .unwrap()
    }

    fn ctl(socket: &Path, words: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .args(["ctl", "--json", "--socket"])
            .arg(socket)
            .args(words)
            .output()
            .unwrap()
    }

    fn query(socket: &Path, words: &[&str]) -> Value {
        let result = ctl(socket, words);
        assert!(
            result.status.success(),
            "{words:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(reply["v"], 1);
        reply["ok"].clone()
    }

    #[test]
    fn every_phase_one_command_reaches_an_isolated_session() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("contexts");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            config.join("context.toml"),
            "app_id = '*'\nbuttons = {}\nspeech = {}\n",
        )
        .unwrap();
        let runtime = root.path().join("runtime");
        let applications = root.path().join("data/applications");
        std::fs::create_dir_all(&applications).unwrap();
        std::fs::write(
            applications.join("org.example.Editor.desktop"),
            "[Desktop Entry]\nType=Application\nName=Example Editor\nExec=true\n",
        )
        .unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let launcher = bin.join("gtk-launch");
        std::fs::write(
            &launcher,
            "#!/bin/sh\nexec /usr/bin/touch \"$(dirname \"$0\")/desktop-launched\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        let driver = runtime.join("tehuti-drive.sock");
        let socket = runtime.join("tehuti-control/ctl");
        let start = Command::new(env!("CARGO_BIN_EXE_tehuti-drive"))
            .arg("--socket")
            .arg(&driver)
            .args([
                "start",
                config.to_str().unwrap(),
                "--headless",
                "--size",
                "320x240",
            ])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("XDG_DATA_DIRS", root.path().join("data"))
            .output()
            .unwrap();
        assert!(
            start.status.success(),
            "{}",
            String::from_utf8_lossy(&start.stderr)
        );
        let checked = std::panic::catch_unwind(|| {
            assert!(query(&socket, &["status"])["pid"].as_u64().is_some());
            assert_eq!(query(&socket, &["windows"]), json!([]));
            assert_eq!(query(&socket, &["workspaces"])[0]["active"], true);
            assert_eq!(query(&socket, &["outputs"])[0]["mode"]["width"], 320);
            query(&socket, &["layout", "focus-workspace-down"]);
            query(&socket, &["notice", "ctl test"]);
            let scene: Value = serde_json::from_slice(&drive(&driver, &["scene", "--json"]).stdout).unwrap();
            assert_eq!(scene["notice"], "ctl test");
            query(&socket, &["display", "scale", "2"]);
            assert_eq!(query(&socket, &["outputs"])[0]["scale"], 2.0);
            query(&socket, &["display", "scale", "1"]);
            query(&socket, &["display", "mode", "640x480@60"]);
            query(&socket, &["display", "mode", "--restore"]);
            assert_eq!(query(&socket, &["outputs"])[0]["mode"]["width"], 320);
            let shot = root.path().join("shot.png");
            query(&socket, &["shot", "output", shot.to_str().unwrap()]);
            assert_eq!(image::image_dimensions(shot).unwrap(), (320, 240));
            assert_eq!(query(&socket, &["context", "list"])[0]["app_id"], "*");
            assert_eq!(query(&socket, &["context", "active"])["global"], true);
            assert_eq!(query(&socket, &["context", "check"])["valid"], true);
            std::fs::write(
                config.join("context.toml"),
                "app_id = 'org.example.Editor'\nbuttons = {}\nspeech = {}\n",
            )
            .unwrap();
            assert_eq!(
                query(&socket, &["context", "reload"])[0]["app_id"],
                "org.example.Editor"
            );
            std::fs::write(config.join("context.toml"), "invalid = '").unwrap();
            assert!(!ctl(&socket, &["context", "check"]).status.success());
            assert!(!ctl(&socket, &["context", "reload"]).status.success());
            assert_eq!(
                query(&socket, &["context", "list"])[0]["app_id"],
                "org.example.Editor"
            );
            assert_eq!(query(&socket, &["plugin", "list"]), json!([]));
            query(&socket, &["launch", "Example Editor"]);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !bin.join("desktop-launched").exists() {
                assert!(Instant::now() < deadline, "desktop application did not launch");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!ctl(&socket, &["launch", "/bin/true"]).status.success());
            assert!(!ctl(&socket, &["launch", "missing.desktop"]).status.success());
            let marker = root.path().join("launched");
            query(
                &socket,
                &["launch", "--command", "/usr/bin/touch", marker.to_str().unwrap()],
            );
            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            for words in [
                &["focus", "999"][..],
                &["close", "999"],
                &["close"],
                &["shot", "window"],
            ] {
                assert!(!ctl(&socket, words).status.success());
            }
            let event_path = root.path().join("events.ndjson");
            let event_process = EventStream(
                Command::new(env!("CARGO_BIN_EXE_tehuti"))
                    .args(["ctl", "--json", "--socket"])
                    .arg(&socket)
                    .arg("events")
                    .stdout(std::fs::File::create(&event_path).unwrap())
                    .spawn()
                    .unwrap(),
            );
            let initial = await_events(&event_path, "\"event\":\"state\"");
            assert_eq!(
                serde_json::from_str::<Value>(initial.lines().next().unwrap()).unwrap()["v"],
                1
            );
            assert!(drive(&driver, &["press", "south"]).status.success());
            let _buttons = await_events(&event_path, "\"kind\":\"button\"");
            assert!(
                drive(&driver, &["say", "private test transcript"])
                    .status
                    .success()
            );
            let speech = await_events(&event_path, "\"kind\":\"speech\"");
            assert!(!speech.contains("private test transcript"));
            drop(event_process);
            malformed_requests_do_not_block(&socket);
            query(&socket, &["session", "restart"]);
        });
        let _stopped = drive(&driver, &["stop"]);
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    }

    fn malformed_requests_do_not_block(socket: &Path) {
        let _idle = UnixStream::connect(socket).unwrap();
        for request in [
            b"{\"v\":2,\"command\":{\"name\":\"status\"}}\n".as_slice(),
            b"{\"v\":1,\"command\":{\"name\":\"key\",\"key\":\"A\"}}\n",
            b"{\"v\":1,\"command\":{\"name\":\"status\",\"secret\":\"hidden\"}}\n",
        ] {
            let mut stream = UnixStream::connect(socket).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            stream.write_all(request).unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).unwrap();
            assert!(
                serde_json::from_str::<Value>(&reply)
                    .unwrap()
                    .get("error")
                    .is_some()
            );
            assert!(!reply.contains("hidden"));
        }
        query(socket, &["status"]);
    }

    #[test]
    fn help_is_available_at_every_command_without_a_session() {
        for command in [
            "status",
            "windows",
            "workspaces",
            "outputs",
            "events",
            "layout",
            "focus",
            "close",
            "launch",
            "notice",
            "display mode",
            "display scale",
            "shot",
            "context list",
            "context active",
            "context reload",
            "context check",
            "plugin list",
            "session restart",
            "volume",
            "record",
            "brightness",
            "update",
            "help-overlay",
            "toggle",
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut words: Vec<_> = command.split_whitespace().collect();
            words.push("--help");
            assert!(
                ctl(&root.path().join("absent"), &words).status.success(),
                "{command}"
            );
        }
        for (command, issue) in [
            ("volume", "918"),
            ("record", "920"),
            ("brightness", "922"),
            ("update", "923"),
            ("help-overlay", "924"),
            ("toggle", "925"),
        ] {
            let words = vec![command.to_owned()];
            assert!(tehuti::ctl_cli::parse(&words).unwrap_err().contains(issue));
        }
    }
}

#[cfg(test)]
#[path = "recording_client/mod.rs"]
mod recording_client;

#[cfg(test)]
mod windows {
    use super::recording_client::Client;
    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        driver::{Driver, Request},
        state::Compositor,
    };

    fn invoke(
        server: &mut tehuti::ctl::Server,
        compositor: &mut Compositor,
        directory: &std::path::Path,
        words: &[&str],
    ) -> serde_json::Value {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .args(["ctl", "--json", "--socket"])
            .arg(directory.join("control/ctl"))
            .args(words)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + core::time::Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            server.poll(|command| compositor.control(command, directory));
            std::thread::sleep(core::time::Duration::from_millis(1));
        }
        let _killed = child.kill();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{words:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    #[test]
    fn window_ids_support_focus_layout_capture_and_close() {
        let directory = tempfile::tempdir().unwrap();
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let first = Client::new(&display, false);
        first.paint(&mut display, &mut compositor, [10, 20, 30, 255]);
        let second = Client::new(&display, false);
        second.paint(&mut display, &mut compositor, [30, 20, 10, 255]);
        let scene = driver.execute(&mut compositor, Request::Scene).unwrap();
        let identity = scene["windows"][0]["id"].as_u64().unwrap();
        let mut server = tehuti::ctl::Server::bind(directory.path().join("control/ctl")).unwrap();
        let windows = invoke(&mut server, &mut compositor, directory.path(), &["windows"]);
        assert!(
            windows["ok"]
                .as_array()
                .unwrap()
                .iter()
                .all(|window| window["app_id"] == "org.example.Game")
        );
        let _reply = invoke(
            &mut server,
            &mut compositor,
            directory.path(),
            &["focus", &identity.to_string()],
        );
        assert_eq!(
            driver.execute(&mut compositor, Request::Scene).unwrap()["focus"],
            identity
        );
        let _reply = invoke(
            &mut server,
            &mut compositor,
            directory.path(),
            &["layout", "toggle-fullscreen"],
        );
        let path = directory.path().join("window.png");
        let _reply = invoke(
            &mut server,
            &mut compositor,
            directory.path(),
            &["shot", "window", path.to_str().unwrap()],
        );
        assert!(image::image_dimensions(path).unwrap().0 > 0);
        let _reply = invoke(
            &mut server,
            &mut compositor,
            directory.path(),
            &["close", &identity.to_string()],
        );
        assert_eq!(
            driver.execute(&mut compositor, Request::Scene).unwrap()["windows"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let _reply = invoke(&mut server, &mut compositor, directory.path(), &["close"]);
        assert_eq!(
            driver.execute(&mut compositor, Request::Scene).unwrap()["windows"],
            serde_json::json!([])
        );
        first.finish();
        second.finish();
    }
}

#[cfg(test)]
mod credentials {
    use core::time::Duration;
    use std::{os::unix::fs::PermissionsExt, process::Command, time::Instant};

    #[test]
    fn a_different_uid_is_rejected_even_with_filesystem_access() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("control");
        let path = private.join("ctl");
        let mut server = tehuti::ctl::Server::bind(path.clone()).unwrap();
        for (accessible, mode) in [
            (directory.path(), 0o755),
            (private.as_path(), 0o755),
            (path.as_path(), 0o666),
        ] {
            std::fs::set_permissions(accessible, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let script = r#"import os,socket,sys
assert os.getuid() != int(sys.argv[2])
stream=socket.socket(socket.AF_UNIX)
stream.settimeout(3)
stream.connect(sys.argv[1])
try:
    stream.sendall(b'{"v":1,"command":{"name":"status"}}\n')
    data=stream.recv(1024)
except (ConnectionResetError,BrokenPipeError):
    data=b''
assert data == b'', data
"#;
        let mut child = Command::new("sudo")
            .args(["-n", "-u", "nobody", "python3", "-c", script])
            .arg(&path)
            .arg(rustix::process::getuid().as_raw().to_string())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut called = false;
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            server.poll(|_| {
                called = true;
                Ok(serde_json::json!({}))
            });
            std::thread::sleep(Duration::from_millis(1));
        }
        let _killed = child.kill();
        assert!(child.wait().unwrap().success());
        assert!(!called, "foreign uid must never reach command dispatch");
    }
}
