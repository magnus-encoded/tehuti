//! Session packaging is exercised with a fake compositor in a temporary home.

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

    #[test]
    fn launcher_sets_session_environment_and_preserves_exit_status() -> Result<(), Box<dyn core::error::Error>>
    {
        let home = tempfile::tempdir()?;
        let fake = home.path().join("fake compositor");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' \"$XDG_CURRENT_DESKTOP\" \"$XDG_SESSION_DESKTOP\" \"$XDG_SESSION_TYPE\" \"${WAYLAND_DISPLAY-unset}\" \"${DISPLAY-unset}\" \"$1\" \"$2\"\nexit 7\n",
        )?;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755))?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(root.join("dist/tehuti-session"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .env("TEHUTI_BINARY", fake)
            .env("WAYLAND_DISPLAY", "host")
            .env("DISPLAY", ":0")
            .arg("application with spaces")
            .status()?;
        assert_eq!(
            status.code(),
            Some(7),
            "the login manager receives the compositor status"
        );
        let log = fs::read_to_string(home.path().join(".local/state/tehuti/session.log"))?;
        let expected = format!(
            "Tehuti\nTehuti\nwayland\nunset\nunset\n{}/.config/tehuti\napplication with spaces\n",
            home.path().display()
        );
        assert_eq!(log, expected, "session environment, arguments and log location");
        Ok(())
    }

    #[test]
    fn launcher_respects_user_config_and_state_directories() -> Result<(), Box<dyn core::error::Error>> {
        let home = tempfile::tempdir()?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = Command::new(root.join("dist/tehuti-session"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .env("TEHUTI_BINARY", "/bin/echo")
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .output()?;
        assert!(output.status.success(), "fake compositor exits successfully");
        assert_eq!(
            fs::read_to_string(home.path().join("state/tehuti/session.log"))?,
            format!("{}/config/tehuti\n", home.path().display()),
            "XDG directories determine context and log paths"
        );
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "compositor output goes to the log"
        );
        Ok(())
    }

    #[test]
    fn installer_copies_only_the_launcher_and_session_entry() -> Result<(), Box<dyn core::error::Error>> {
        let destination = tempfile::tempdir()?;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = Command::new(root.join("scripts/install-session"))
            .env("DESTDIR", destination.path())
            .output()?;
        assert!(output.status.success(), "temporary session install succeeds");
        for relative in [
            "usr/local/bin/tehuti-session",
            "usr/share/wayland-sessions/tehuti.desktop",
        ] {
            assert_eq!(
                fs::read(destination.path().join(relative))?,
                fs::read(root.join(if relative.ends_with(".desktop") {
                    "dist/tehuti.desktop"
                } else {
                    "dist/tehuti-session"
                }))?,
                "installed file matches repository"
            );
        }
        let repeat = Command::new(root.join("scripts/install-session"))
            .env("DESTDIR", destination.path())
            .output()?;
        assert!(!repeat.status.success(), "existing session files are preserved");
        assert!(
            String::from_utf8(output.stdout)?.contains("Remove with:"),
            "owner gets removal instructions"
        );
        Ok(())
    }
}
