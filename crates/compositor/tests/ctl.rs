#[cfg(test)]
mod tests {
    use core::time::Duration;
    use serde_json::{Value, json};
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixStream},
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
        let deadline = Instant::now().checked_add(Duration::from_secs(5)).unwrap();
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
        ctl_from(socket, words, &std::env::current_dir().unwrap())
    }

    fn ctl_from(socket: &Path, words: &[&str], directory: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tehuti"))
            .current_dir(directory)
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
    #[expect(
        clippy::too_many_lines,
        reason = "one integration scenario owns the isolated session and its cleanup"
    )]
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
            assert_eq!(query(&socket, &["workspaces"])[0]["focused"], true);
            assert_eq!(query(&socket, &["outputs"])[0]["mode"]["width"], 320);
            let _workspace_change = query(&socket, &["layout", "focus-workspace-down"]);
            let _notice = query(&socket, &["notice", "ctl test"]);
            let scene: Value = serde_json::from_slice(&drive(&driver, &["scene", "--json"]).stdout).unwrap();
            assert_eq!(scene["notice"], "ctl test");
            let _scale_up = query(&socket, &["display", "scale", "2"]);
            assert_eq!(query(&socket, &["outputs"])[0]["scale"], 2.0);
            let _scale_restore = query(&socket, &["display", "scale", "1"]);
            let _mode_change = query(&socket, &["display", "mode", "640x480@60"]);
            let _mode_restore = query(&socket, &["display", "mode", "--restore"]);
            assert_eq!(query(&socket, &["outputs"])[0]["mode"]["width"], 320);
            let caller = root.path().join("caller");
            std::fs::create_dir(&caller).unwrap();
            for words in [&["shot", "shot.png"][..], &["shot", "output", "output.png"]] {
                assert!(ctl_from(&socket, words, &caller).status.success());
                assert_eq!(
                    image::image_dimensions(caller.join(words.last().unwrap())).unwrap(),
                    (320, 240)
                );
            }
            let caller_contexts = caller.join("contexts");
            std::fs::create_dir(&caller_contexts).unwrap();
            std::fs::write(
                caller_contexts.join("context.toml"),
                "app_id = '*'\nbuttons = {}\nspeech = {}\n",
            )
            .unwrap();
            assert!(
                ctl_from(&socket, &["context", "check", "contexts"], &caller)
                    .status
                    .success()
            );
            std::fs::write(caller_contexts.join("context.toml"), "invalid = '").unwrap();
            assert!(
                !ctl_from(&socket, &["context", "check", "contexts"], &caller)
                    .status
                    .success()
            );
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
            let _desktop_launch = query(&socket, &["launch", "Example Editor"]);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !bin.join("desktop-launched").exists() {
                assert!(Instant::now() < deadline, "desktop application did not launch");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!ctl(&socket, &["launch", "/bin/true"]).status.success());
            assert!(!ctl(&socket, &["launch", "missing.desktop"]).status.success());
            let marker = root.path().join("launched");
            let _command_launch = query(
                &socket,
                &["launch", "--command", "/usr/bin/touch", marker.to_str().unwrap()],
            );
            let launch_deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() {
                assert!(Instant::now() < launch_deadline);
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
            let _barrier_notice = query(&socket, &["notice", "event stream barrier"]);
            let changed = await_events(&event_path, "event stream barrier");
            let records: Vec<Value> = changed
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            for pair in records.windows(2) {
                assert_eq!(
                    pair[1]["sequence"].as_u64().unwrap(),
                    pair[0]["sequence"].as_u64().unwrap() + 1
                );
            }
            assert!(records.iter().all(|record| {
                record["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|event| serde_json::from_value::<tehuti::ctl::Event>(event.clone()).is_ok())
            }));
            drop(event_process);
            malformed_requests_do_not_block(&socket);
            let _restart = query(&socket, &["session", "restart"]);
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
            let _read = stream.read_to_string(&mut reply).unwrap();
            assert!(
                serde_json::from_str::<Value>(&reply)
                    .unwrap()
                    .get("error")
                    .is_some()
            );
            assert!(!reply.contains("hidden"));
        }
        let _status = query(socket, &["status"]);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one integration scenario owns the isolated session and its cleanup"
    )]
    fn pad_input_is_streamed_and_keyboard_input_yields_only_a_summary() {
        let directory = tempfile::tempdir().unwrap();
        let display =
            smithay::reexports::wayland_server::Display::<tehuti::state::Compositor>::new().unwrap();
        let mut compositor = tehuti::state::Compositor::new(&display.handle()).unwrap();
        let mut driver =
            tehuti::driver::Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let initial = compositor
            .control(tehuti::ctl::Command::Events, directory.path())
            .unwrap();
        let _pad_button = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Press {
                    button: tehuti_protocol::ButtonName::South,
                },
            )
            .unwrap();
        let _stick_sample = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Stick {
                    left: (500, -500),
                    right: (-500, 500),
                },
            )
            .unwrap();
        let outside = compositor
            .control(tehuti::ctl::Command::Events, directory.path())
            .unwrap();
        let outside_events = outside.as_array().unwrap();
        assert_eq!(outside_events.len(), initial.as_array().unwrap().len() + 3);
        assert_eq!(outside_events[outside_events.len() - 3]["kind"], "button");
        assert_eq!(outside_events[outside_events.len() - 2]["kind"], "button");
        assert_eq!(outside_events[outside_events.len() - 1]["kind"], "sticks");
        let _keyboard_open = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Say {
                    text: "keyboard".into(),
                },
            )
            .unwrap();
        assert!(compositor.on_screen_keyboard().is_open());
        let opened = compositor
            .control(tehuti::ctl::Command::Events, directory.path())
            .unwrap();
        for button in [
            tehuti_protocol::ButtonName::DpadRight,
            tehuti_protocol::ButtonName::South,
        ] {
            let _keyboard_button = driver
                .execute(&mut compositor, tehuti::driver::Request::Press { button })
                .unwrap();
        }
        for _ in 0..2 {
            let _layer = driver
                .execute(
                    &mut compositor,
                    tehuti::driver::Request::Press {
                        button: tehuti_protocol::ButtonName::LeftStick,
                    },
                )
                .unwrap();
        }
        let _rest = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Stick {
                    left: (0, 0),
                    right: (0, 0),
                },
            )
            .unwrap();
        let _word = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Stick {
                    left: (-1000, 0),
                    right: (0, 0),
                },
            )
            .unwrap();
        let _delete_word = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Press {
                    button: tehuti_protocol::ButtonName::West,
                },
            )
            .unwrap();
        let _text_entry = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Text {
                    text: "private test text".into(),
                },
            )
            .unwrap();
        assert_eq!(
            compositor
                .control(tehuti::ctl::Command::Events, directory.path())
                .unwrap(),
            opened
        );
        let _close = driver
            .execute(
                &mut compositor,
                tehuti::driver::Request::Press {
                    button: tehuti_protocol::ButtonName::East,
                },
            )
            .unwrap();
        assert!(!compositor.on_screen_keyboard().is_open());
        let closed = compositor
            .control(tehuti::ctl::Command::Events, directory.path())
            .unwrap();
        let closed_events = closed.as_array().unwrap();
        assert_eq!(closed_events.len(), opened.as_array().unwrap().len() + 1);
        assert_eq!(
            closed_events.last().unwrap(),
            &json!({
                "kind": "keyboard_summary", "sequence": 4,
                "inputs": {"letters": 11, "capitals": 0, "symbols": 2},
                "character_edits": 1, "word_edits": 1,
            })
        );
        assert!(!closed.to_string().contains("private test text"));
        for kind in ["key", "text", "speech"] {
            assert!(serde_json::from_value::<tehuti::ctl::Event>(json!({"kind": kind})).is_err());
        }
    }

    #[test]
    fn headless_session_answers_control_without_a_host_display() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("contexts");
        std::fs::create_dir(&config).unwrap();
        let mut session = EventStream(
            Command::new(env!("CARGO_BIN_EXE_tehuti"))
                .arg("--test-driver")
                .arg(&config)
                .env("XDG_RUNTIME_DIR", root.path())
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("XDG_DATA_HOME", root.path().join("data"))
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("DISPLAY")
                .env_remove("TEHUTI_SAY")
                .env_remove("TEHUTI_SPEECH_SIDECAR")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let socket = root.path().join("tehuti-control/ctl");
        let deadline = Instant::now().checked_add(Duration::from_secs(5)).unwrap();
        while !socket.exists() {
            assert!(session.0.try_wait().unwrap().is_none(), "headless session exited");
            assert!(Instant::now() < deadline, "headless session did not bind control");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(query(&socket, &["status"])["pid"], session.0.id());
        assert!(query(&socket, &["outputs"])[0]["mode"]["width"].as_i64().unwrap() > 0);
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
            .current_dir(directory)
            .args(words)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now()
            .checked_add(core::time::Duration::from_secs(5))
            .unwrap();
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

    fn open_keyboard(driver: &mut Driver, compositor: &mut Compositor) {
        let _opened = driver
            .execute(
                compositor,
                Request::Say {
                    text: "keyboard".into(),
                },
            )
            .unwrap();
        assert!(compositor.on_screen_keyboard().is_open());
    }

    fn keyboard_summaries(
        compositor: &mut Compositor,
        directory: &std::path::Path,
    ) -> Vec<serde_json::Value> {
        compositor
            .control(tehuti::ctl::Command::Events, directory)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "keyboard_summary")
            .cloned()
            .collect()
    }

    fn keyboard_summary_on_hide(hide: impl FnOnce(&mut Compositor, &std::path::Path)) {
        let directory = tempfile::tempdir().unwrap();
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let mut driver = Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let client = Client::new(&display, false);
        client.paint(&mut display, &mut compositor, [10, 20, 30, 255]);
        open_keyboard(&mut driver, &mut compositor);
        let _typed = driver
            .execute(
                &mut compositor,
                Request::Press {
                    button: tehuti_protocol::ButtonName::South,
                },
            )
            .unwrap();
        hide(&mut compositor, directory.path());
        assert!(!compositor.on_screen_keyboard().is_open());
        assert_eq!(compositor.on_screen_keyboard().edit_counts(), (0, 0));
        let first = keyboard_summaries(&mut compositor, directory.path());
        assert_eq!(first.len(), 1);
        assert_eq!(first[0]["character_edits"], 1);
        assert_eq!(first[0]["word_edits"], 0);
        assert_eq!(first[0]["inputs"]["letters"], 2);
        open_keyboard(&mut driver, &mut compositor);
        let _closed = driver
            .execute(
                &mut compositor,
                Request::Press {
                    button: tehuti_protocol::ButtonName::East,
                },
            )
            .unwrap();
        let both = keyboard_summaries(&mut compositor, directory.path());
        assert_eq!(both.len(), 2);
        assert_eq!(both[1]["character_edits"], 0);
        assert_eq!(both[1]["word_edits"], 0);
        assert_eq!(both[1]["inputs"]["letters"], 1);
        let _tick = driver
            .execute(&mut compositor, Request::Advance { milliseconds: 100 })
            .unwrap();
        assert_eq!(keyboard_summaries(&mut compositor, directory.path()), both);
        client.finish();
    }

    #[test]
    fn focus_loss_emits_one_keyboard_summary_and_resets_counts() {
        keyboard_summary_on_hide(|compositor, _| {
            compositor.handle(&tehuti_protocol::Event::Focus {
                app_id: None,
                title: String::new(),
            });
        });
    }

    #[test]
    fn window_close_emits_one_keyboard_summary_and_resets_counts() {
        keyboard_summary_on_hide(|compositor, directory| {
            let _closed = compositor
                .control(tehuti::ctl::Command::Close { identity: None }, directory)
                .unwrap();
        });
    }

    #[test]
    fn session_end_emits_one_keyboard_summary_and_resets_counts() {
        keyboard_summary_on_hide(|compositor, directory| {
            let _ended = compositor
                .control(tehuti::ctl::Command::SessionRestart, directory)
                .unwrap();
        });
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one integration scenario owns the isolated session and its cleanup"
    )]
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
        let _focused = invoke(
            &mut server,
            &mut compositor,
            directory.path(),
            &["focus", &identity.to_string()],
        );
        assert_eq!(
            driver.execute(&mut compositor, Request::Scene).unwrap()["focus"],
            identity
        );
        let _layout = invoke(
            &mut server,
            &mut compositor,
            directory.path(),
            &["layout", "toggle-fullscreen"],
        );
        let status = invoke(&mut server, &mut compositor, directory.path(), &["status"]);
        assert_eq!(status["ok"]["id"], identity);
        assert_eq!(status["ok"]["focused"], true);
        assert!(status["ok"].get("focus").is_none());
        let path = directory.path().join("window.png");
        let _capture = invoke(
            &mut server,
            &mut compositor,
            directory.path(),
            &["shot", "window", "window.png"],
        );
        assert!(image::image_dimensions(path).unwrap().0 > 0);
        let _closed_window = invoke(
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
        let _closed_focused = invoke(&mut server, &mut compositor, directory.path(), &["close"]);
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
        let deadline = Instant::now().checked_add(Duration::from_secs(5)).unwrap();
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
