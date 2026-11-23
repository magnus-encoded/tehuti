//! Terminal restoration keeps the persistent identity and only attaches it.

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, process::Command};
    use tehuti::session_restore::Saved;

    const SAVED: &str = r#"{"version":1,"windows":[{"app_id":"org.example.Terminal.desktop","workspace":0,"column":0,"terminal_session":{"socket":"/run/example/mux","session":"$7"}}],"focused":0}"#;

    #[test]
    fn persistent_terminal_identity_round_trips() {
        let saved = Saved::parse(SAVED).unwrap();
        let value: serde_json::Value = serde_json::from_str(&saved.to_text()).unwrap();
        assert_eq!(value["windows"][0]["terminal_session"]["session"], "$7");
        assert_eq!(
            value["windows"][0]["terminal_session"]["socket"],
            "/run/example/mux"
        );
    }

    #[test]
    fn restoring_a_terminal_only_attaches_the_existing_session() {
        if std::env::var_os("TEHUTI_TERMINAL_RESTORE_PROBE").is_some() {
            let display =
                smithay::reexports::wayland_server::Display::<tehuti::state::Compositor>::new().unwrap();
            let mut compositor = tehuti::state::Compositor::new(&display.handle()).unwrap();
            compositor.set_launch_socket("restore-test".into());
            compositor.begin_session_restore(&Saved::parse(SAVED).unwrap());
            let trace = std::path::PathBuf::from(std::env::var_os("TEHUTI_RESTORE_TRACE").unwrap());
            let start = std::time::Instant::now();
            while !trace.exists() && start.elapsed() < core::time::Duration::from_secs(5) {
                std::thread::sleep(core::time::Duration::from_millis(10));
            }
            assert_eq!(
                fs::read_to_string(trace).unwrap(),
                "terminal\n--app-id=org.example.Terminal.desktop\n--\ntmux\n-S\n/run/example/mux\nattach-session\n-t\n$7\n"
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let applications = root.path().join("applications");
        fs::create_dir(&applications).unwrap();
        fs::write(
            applications.join("org.example.Terminal.desktop"),
            "[Desktop Entry]\nName=Terminal\nType=Application\nExec=terminal\n",
        )
        .unwrap();
        for (name, label) in [("gtk-launch", "replacement"), ("xdg-terminal-exec", "terminal")] {
            let path = root.path().join(name);
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '%s\\n' '{label}' \"$@\" >\"$TEHUTI_RESTORE_TRACE\"\n"),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::restoring_a_terminal_only_attaches_the_existing_session",
                    "--nocapture"
                ])
                .env("TEHUTI_TERMINAL_RESTORE_PROBE", "1")
                .env("TEHUTI_RESTORE_TRACE", root.path().join("trace"))
                .env("XDG_DATA_HOME", root.path())
                .env("XDG_DATA_DIRS", root.path())
                .env("XDG_CONFIG_HOME", root.path())
                .env("PATH", root.path())
                .status()
                .unwrap()
                .success()
        );
    }
}
