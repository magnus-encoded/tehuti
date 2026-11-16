//! Registers the running session with the systemd user manager and the D-Bus activation environment.

use std::{ffi::OsStr, io};

/// The target the compositor starts once its socket listens; it binds `graphical-session.target`.
pub const TARGET: &str = "tehuti-session.target";

/// A command the compositor runs to register or deregister the session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub program: &'static str,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
}

const NAMES: [&str; 5] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
];

fn invocation(program: &'static str, args: &[&str]) -> Invocation {
    Invocation {
        program,
        args: args.iter().map(|argument| (*argument).to_owned()).collect(),
        envs: Vec::new(),
    }
}

/// Commands that import the session environment and start the session target, in order.
#[must_use]
pub fn start_plan(socket: &OsStr, display: Option<&OsStr>) -> Vec<Invocation> {
    let values = [
        ("WAYLAND_DISPLAY", Some(socket.to_string_lossy().into_owned())),
        (
            "DISPLAY",
            display.map(|value| value.to_string_lossy().into_owned()),
        ),
        ("XDG_CURRENT_DESKTOP", Some("Tehuti".to_owned())),
        ("XDG_SESSION_TYPE", Some("wayland".to_owned())),
        ("XDG_SESSION_DESKTOP", Some("Tehuti".to_owned())),
    ];
    let envs: Vec<(String, String)> = values
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value)))
        .collect();
    let names: Vec<&str> = envs.iter().map(|(name, _)| name.as_str()).collect();
    let mut import = invocation("systemctl", &["--user", "import-environment"]);
    import.args.extend(names.iter().map(|name| (*name).to_owned()));
    import.envs.clone_from(&envs);
    let mut activation = invocation("dbus-update-activation-environment", &[]);
    activation
        .args
        .extend(names.iter().map(|name| (*name).to_owned()));
    activation.envs = envs;
    vec![
        import,
        activation,
        invocation("systemctl", &["--user", "start", TARGET]),
    ]
}

/// Commands that stop the session target and clear the imported names.
#[must_use]
pub fn stop_plan() -> Vec<Invocation> {
    let mut unset = invocation("systemctl", &["--user", "unset-environment"]);
    unset.args.extend(NAMES.map(str::to_owned));
    vec![invocation("systemctl", &["--user", "stop", TARGET]), unset]
}

/// Runs every step, whatever earlier ones did, and describes each failure for the log.
pub fn report(plan: &[Invocation], runner: &mut dyn FnMut(&Invocation) -> io::Result<()>) -> Vec<String> {
    plan.iter()
        .filter_map(|step| {
            runner(step)
                .err()
                .map(|error| format!("{} {}: {error}", step.program, step.args.join(" ")))
        })
        .collect()
}

/// Runs one command to completion; a missing program or non-zero exit is an error.
///
/// # Errors
/// The program cannot start or exits unsuccessfully.
pub fn execute(step: &Invocation) -> io::Result<()> {
    let status = std::process::Command::new(step.program)
        .args(&step.args)
        .envs(step.envs.iter().map(|(key, value)| (key, value)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("exited with {status}")))
    }
}

/// Holds the registration; dropping it deregisters the session.
#[derive(Debug)]
pub struct Registration(());

impl Registration {
    /// Registers the session, passing each failure to `log`; never fails.
    pub fn start(socket: &OsStr, display: Option<&OsStr>, log: &mut dyn FnMut(&str)) -> Self {
        for warning in report(&start_plan(socket, display), &mut |step| execute(step)) {
            log(&warning);
        }
        Self(())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _warnings = report(&stop_plan(), &mut |step| execute(step));
    }
}
