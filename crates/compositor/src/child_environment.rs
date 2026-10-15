//! Session display variables applied without mutating the process environment.
use std::{ffi::OsString, process::Command, sync::RwLock};

static DISPLAY: RwLock<Option<OsString>> = RwLock::new(None);

pub(crate) fn set_display(display: OsString) {
    *DISPLAY.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(display);
}

/// Builds a child command with the current Xwayland display.
#[must_use]
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    if let Some(display) = &*DISPLAY.read().unwrap_or_else(std::sync::PoisonError::into_inner) {
        let _command = command.env("DISPLAY", display);
    }
    command
}

pub(crate) fn export(display: &str) {
    for (program, arguments) in [
        (
            "systemctl",
            vec![
                "--user".to_owned(),
                "set-environment".to_owned(),
                format!("DISPLAY={display}"),
            ],
        ),
        (
            "dbus-update-activation-environment",
            vec![format!("DISPLAY={display}")],
        ),
    ] {
        match command(program).args(arguments).spawn() {
            Ok(mut child) => {
                let _reaper = std::thread::spawn(move || match child.wait() {
                    Ok(status) if status.success() => {}
                    result => tracing::warn!(?result, %program, "display environment export failed"),
                });
            }
            Err(error) => tracing::warn!(%error, %program, "display environment export failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spawned_children_receive_the_session_display() {
        super::set_display(":91".into());
        let output = super::command("sh")
            .args(["-c", "printf '%s' \"$DISPLAY\""])
            .output()
            .unwrap();
        assert!(output.status.success(), "child exits successfully");
        assert_eq!(output.stdout, b":91", "child receives Xwayland DISPLAY");
        *super::DISPLAY.write().unwrap() = None;
    }
}
