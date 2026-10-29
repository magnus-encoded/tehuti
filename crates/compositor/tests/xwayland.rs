#[cfg(test)]
mod xwayland {
    use std::path::PathBuf;
    use tehuti::xwayland::find_binary;

    proptest::proptest! {
        #[test]
        fn selected_display_is_free_and_every_lower_number_is_occupied(
            occupied in proptest::collection::btree_set(0_u16..1024, 0..1025)
        ) {
            if let Some(number) = tehuti::xwayland::choose_display(|number| occupied.contains(&number)) {
                proptest::prop_assert!(!occupied.contains(&number));
                proptest::prop_assert!((0..number).all(|lower| occupied.contains(&lower)));
            } else {
                proptest::prop_assert_eq!(occupied.len(), 1024);
            }
        }
    }

    #[test]
    fn binary_discovery_uses_the_first_executable_in_path_order() {
        let paths = [PathBuf::from("/opt/first"), PathBuf::from("/opt/second")];
        assert_eq!(
            find_binary(&paths, |path| path
                == std::path::Path::new("/opt/second/xwayland-satellite")),
            Some(PathBuf::from("/opt/second/xwayland-satellite"))
        );
        assert_eq!(
            find_binary(&paths, |_| true),
            Some(PathBuf::from("/opt/first/xwayland-satellite"))
        );
        assert_eq!(find_binary(&paths, |_| false), None);
        assert_eq!(find_binary(&[], |_| true), None);
    }

    #[test]
    fn display_selection_skips_occupied_numbers_and_reports_exhaustion() {
        assert_eq!(
            tehuti::xwayland::choose_display(|number| [0, 1, 3].contains(&number)),
            Some(2)
        );
        assert_eq!(tehuti::xwayland::choose_display(|_| false), Some(0));
        assert_eq!(tehuti::xwayland::choose_display(|_| true), None);
    }

    #[test]
    fn restart_policy_backs_off_caps_and_recovers_after_a_minute_alive() {
        use core::time::Duration;
        use tehuti::child_process::Restart;
        let mut restart = Restart::default();
        assert!(restart.ready());
        for seconds in [0, 1, 2, 5, 10, 20, 30, 30] {
            restart.failed();
            if seconds > 0 {
                assert!(!restart.ready());
                restart.advance(Duration::from_millis(seconds * 1000 - 1), false);
                assert!(!restart.ready());
                restart.advance(Duration::from_millis(1), false);
            }
            assert!(restart.ready());
        }
        restart.advance(Duration::from_secs(59), true);
        restart.failed();
        assert!(!restart.ready());
        restart.advance(Duration::from_secs(30), false);
        restart.advance(Duration::from_secs(60), true);
        restart.failed();
        assert!(restart.ready());
    }

    #[test]
    fn child_environment_overrides_display_and_removes_it_in_wayland_only_mode() {
        let mut command = std::process::Command::new("application");
        let _command = command.env("DISPLAY", ":99");
        tehuti::child_process::set_display(&mut command, Some(std::ffi::OsStr::new(":2")));
        assert_eq!(
            command.get_envs().find(|(key, _)| *key == "DISPLAY").unwrap().1,
            Some(std::ffi::OsStr::new(":2"))
        );
        tehuti::child_process::set_display(&mut command, None);
        assert_eq!(
            command.get_envs().find(|(key, _)| *key == "DISPLAY").unwrap().1,
            None
        );
    }
    #[test]
    fn satellite_restarts_on_the_same_display_and_is_reaped_on_drop() {
        use core::time::Duration;
        use std::{os::unix::fs::PermissionsExt as _, time::Instant};
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("satellite");
        let marker = directory.path().join("starts");
        std::fs::write(&binary, format!(
            "#!/usr/bin/env python3\nimport os,pathlib,sys,time\nmarker=pathlib.Path({:?})\nwith marker.open('a') as output: output.write(sys.argv[1]+' '+os.environ['WAYLAND_DISPLAY']+' '+os.environ['DISPLAY']+' '+str(os.getpid())+'\\n')\nif len(marker.read_text().splitlines()) == 1: raise SystemExit(7)\ntime.sleep(60)\n",
            marker.to_string_lossy()
        )).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        tehuti::child_process::configure_display(Some(":42".into())).unwrap();
        let mut satellite = tehuti::xwayland::Satellite::new(binary, ":42".into(), "wayland-test".into());
        let deadline = Instant::now().checked_add(Duration::from_secs(5)).unwrap();
        let lines = loop {
            satellite.poll(Duration::from_millis(10));
            let lines = std::fs::read_to_string(&marker).unwrap_or_default();
            if lines.lines().count() == 2 {
                break lines;
            }
            assert!(Instant::now() < deadline, "satellite did not restart: {lines}");
            std::thread::sleep(Duration::from_millis(10));
        };
        for line in lines.lines() {
            assert!(
                line.starts_with(":42 wayland-test :42 "),
                "wrong child environment: {line}"
            );
        }
        let pid = lines.lines().last().unwrap().split_whitespace().last().unwrap();
        drop(satellite);
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "satellite was not reaped"
        );
    }

    #[test]
    fn discovery_and_child_environments_work_without_inherited_x11_state() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        for mode in ["missing", "installed", "wayland", "x11"] {
            if mode == "installed" {
                let binary = directory.path().join("xwayland-satellite");
                std::fs::write(&binary, "#!/bin/sh\nexit 0\n").unwrap();
                std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "xwayland::isolated_session_probe", "--nocapture"])
                .env("TEHUTI_XWAYLAND_PROBE", mode)
                .env("PATH", directory.path())
                .env("DISPLAY", ":99")
                .env("WAYLAND_DISPLAY", "parent-socket")
                .output()
                .unwrap();
            let output = String::from_utf8_lossy(&result.stdout);
            assert!(
                result.status.success(),
                "{mode}: {output} {}",
                String::from_utf8_lossy(&result.stderr)
            );
            if mode == "missing" {
                assert_eq!(output.matches("xwayland-satellite is not installed").count(), 1);
            }
        }
    }

    #[test]
    fn isolated_session_probe() {
        let Ok(mode) = std::env::var("TEHUTI_XWAYLAND_PROBE") else {
            return;
        };
        match mode.as_str() {
            "missing" => {
                tracing_subscriber::fmt()
                    .with_writer(std::io::stdout)
                    .try_init()
                    .unwrap();
                assert!(tehuti::xwayland::discover().is_none());
            }
            "installed" => {
                let (binary, display) = tehuti::xwayland::discover().unwrap();
                assert_eq!(
                    binary,
                    std::path::Path::new(&std::env::var_os("PATH").unwrap()).join("xwayland-satellite")
                );
                assert!(display.starts_with(':'));
            }
            "wayland" | "x11" => {
                let display = (mode == "x11").then(|| ":42".into());
                tehuti::child_process::configure_display(display).unwrap();
                let expected = if mode == "x11" { ":42" } else { "unset" };
                let output = tehuti::child_process::command("/bin/sh")
                    .args(["-c", "printf '%s' \"${DISPLAY-unset}\""])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
                let client_output = tehuti::client_env::command(
                    std::ffi::OsStr::new("/bin/sh"),
                    std::ffi::OsStr::new("session-socket"),
                )
                .args(["-c", "printf '%s %s' \"${DISPLAY-unset}\" \"$WAYLAND_DISPLAY\""])
                .output()
                .unwrap();
                assert!(client_output.status.success());
                assert_eq!(
                    String::from_utf8(client_output.stdout).unwrap(),
                    format!("{expected} session-socket")
                );
                assert!(tehuti::child_process::configure_display(None).is_err());
            }
            _ => panic!("unknown session probe"),
        }
    }

    #[test]
    fn failed_satellite_start_is_contained_and_retried() {
        use core::time::Duration;
        use std::{os::unix::fs::PermissionsExt as _, time::Instant};
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("satellite");
        let marker = directory.path().join("satellite.started");
        let mut satellite =
            tehuti::xwayland::Satellite::new(binary.clone(), ":43".into(), "wayland-test".into());
        satellite.poll(Duration::ZERO);
        satellite.poll(Duration::ZERO);
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf started > \"$0.started\"\nexec /bin/sleep 60\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        satellite.poll(Duration::from_secs(1));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&marker).is_ok_and(|text| text == "started") {
            assert!(
                Instant::now() < deadline,
                "satellite did not recover from failed start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "started");
    }
}
