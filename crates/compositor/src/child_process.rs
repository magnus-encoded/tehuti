//! Shared sidecar restart policy and child environment.

use core::time::Duration;

#[derive(Debug, Default)]
pub struct Restart {
    retry: Duration,
    failures: usize,
    age: Duration,
}

impl Restart {
    /// Advances the retry clock or the current child's healthy lifetime.
    pub fn advance(&mut self, elapsed: Duration, running: bool) {
        self.retry = self.retry.saturating_sub(elapsed);
        if running {
            self.age = self.age.saturating_add(elapsed);
            if self.age >= Duration::from_secs(60) {
                self.failures = 0;
            }
        }
    }

    /// Schedules another attempt using the speech sidecar's capped backoff.
    pub fn failed(&mut self) {
        self.retry = Duration::from_secs([0, 1, 2, 5, 10, 20].get(self.failures).copied().unwrap_or(30));
        self.failures = self.failures.saturating_add(1);
        self.age = Duration::ZERO;
    }

    #[must_use]
    pub const fn ready(&self) -> bool {
        self.retry.is_zero()
    }
}

static DISPLAY: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();

/// Sets the session's child display before launching sidecars or applications.
///
/// # Errors
/// The session display was already configured.
pub fn configure_display(display: Option<std::ffi::OsString>) -> std::io::Result<()> {
    DISPLAY
        .set(display)
        .map_err(|_| std::io::Error::other("child display already configured"))
}

/// Applies the session display, clearing inherited X11 state in Wayland-only sessions.
pub fn set_display(command: &mut std::process::Command, display: Option<&std::ffi::OsStr>) {
    if let Some(display) = display {
        let _command = command.env("DISPLAY", display);
    } else {
        let _command = command.env_remove("DISPLAY");
    }
}

/// Constructs a child command with the configured session display.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    if let Some(display) = DISPLAY.get() {
        set_display(&mut command, display.as_deref());
    }
    command
}
