//! Launcher process boundary with an isolated executable and environment.

#[cfg(test)]
mod desktop_launch {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn launch_passes_the_entry_id_and_compositor_socket_without_shell_expansion() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("gtk-launch");
        let capture = directory.path().join("capture");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$WAYLAND_DISPLAY\" \"$DISPLAY\" \"$@\" > \"$TEHUTI_LAUNCH_CAPTURE\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "desktop_launch::isolated_launch_probe"])
            .env("PATH", directory.path())
            .env("TEHUTI_LAUNCH_CAPTURE", &capture)
            .env("DISPLAY", ":99")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "launcher probe failed: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(
            std::fs::read_to_string(capture).unwrap(),
            "tehuti-test-socket\n:42\norg.example;echo injected.desktop\n"
        );
    }

    #[test]
    fn isolated_launch_probe() {
        if std::env::var_os("TEHUTI_LAUNCH_CAPTURE").is_none() {
            return;
        }
        tehuti::child_process::configure_display(Some(":42".into())).unwrap();
        let mut child = tehuti::desktop::launch(
            "org.example;echo injected.desktop",
            std::ffi::OsStr::new("tehuti-test-socket"),
        )
        .unwrap();
        assert!(child.wait().unwrap().success(), "launcher finished successfully");
    }
}
