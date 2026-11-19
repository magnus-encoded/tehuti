//! The one owner of child processes Tehuti starts for the user: start, run, exit, crash
//! and the notice shown when a child closes right away.

use core::time::Duration;
use std::{
    ffi::OsString,
    io,
    process::{Child, Stdio},
    time::Instant,
};

/// How long after launch a failed exit counts as "closed right away".
const EARLY_EXIT: Duration = Duration::from_secs(5);
/// Children tracked at once; further launches are refused.
const CAPACITY: usize = 16;

/// What to start: the program, its arguments and the app id its window will carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    program: OsString,
    args: Vec<OsString>,
    label: String,
    app_id: Option<String>,
    socket: Option<OsString>,
}

impl Launch {
    #[must_use]
    pub fn new(program: impl Into<OsString>, label: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            label: label.into(),
            app_id: None,
            socket: None,
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// The app id the launched window is given (for example through a `--class`
    /// or `--app-id` argument), so a specialised launch gets its own context.
    #[must_use]
    pub fn with_app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = Some(app_id.into());
        self
    }

    /// Connects the child to this compositor's Wayland socket.
    #[must_use]
    pub fn on_socket(mut self, socket: impl Into<OsString>) -> Self {
        self.socket = Some(socket.into());
        self
    }

    #[must_use]
    pub fn labelled(mut self, label: &str) -> Self {
        label.clone_into(&mut self.label);
        self
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn app_id(&self) -> Option<&str> {
        self.app_id.as_deref()
    }

    /// The command with the session environment and detached standard streams.
    #[must_use]
    pub fn command(&self) -> std::process::Command {
        let mut command = match &self.socket {
            Some(socket) => crate::client_env::command(&self.program, socket),
            None => crate::child_process::command(&self.program),
        };
        let _command = command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
}

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Success,
    Code(i32),
    /// Ended without an exit code, for example killed by a signal.
    Crashed,
}

/// A running child, as far as the owner needs to know.
pub trait Process {
    /// # Errors
    /// The child cannot be waited on.
    fn try_wait(&mut self) -> io::Result<Option<Exit>>;
    fn terminate(&mut self);
}

/// Starts children; the system spawner runs real processes, [`FakeSpawner`] scripts them.
pub trait Spawner {
    type Process: Process;
    /// # Errors
    /// The program cannot start.
    fn spawn(&mut self, launch: &Launch) -> io::Result<Self::Process>;
}

impl Process for Child {
    fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        Ok(
            Child::try_wait(self)?.map(|status| match (status.success(), status.code()) {
                (true, _) => Exit::Success,
                (false, Some(code)) => Exit::Code(code),
                (false, None) => Exit::Crashed,
            }),
        )
    }

    fn terminate(&mut self) {
        crate::shutdown::terminate(self, false);
    }
}

#[derive(Debug, Default)]
pub struct SystemSpawner;

impl Spawner for SystemSpawner {
    type Process = Child;

    fn spawn(&mut self, launch: &Launch) -> io::Result<Child> {
        launch.command().spawn()
    }
}

/// A launched child that ended unsuccessfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub label: String,
    pub app_id: Option<String>,
    pub code: Option<i32>,
    /// Ended within a few seconds of the launch.
    pub early: bool,
}

impl Failure {
    /// The bar notice for an early exit.
    #[must_use]
    pub fn notice(&self) -> String {
        let exit = self.code.map_or_default(|code| format!(" (exit {code})"));
        format!("{} closed right away{exit}", self.label)
    }

    /// One notice for every early exit seen in a poll, `None` when none ended early.
    #[must_use]
    pub fn summary(failures: &[Self]) -> Option<String> {
        let early: Vec<&Self> = failures.iter().filter(|failure| failure.early).collect();
        match early.as_slice() {
            [] => None,
            [only] => Some(only.notice()),
            many => {
                let names: Vec<&str> = many.iter().map(|failure| failure.label.as_str()).collect();
                Some(format!(
                    "{} apps closed right away: {}",
                    many.len(),
                    names.join(", ")
                ))
            }
        }
    }
}

#[derive(Debug)]
struct Running<P> {
    process: P,
    label: String,
    app_id: Option<String>,
    started: Instant,
}

/// Owns every launched child until it exits; dropping it terminates the rest.
#[derive(Debug)]
pub struct Launches<S: Spawner = SystemSpawner> {
    spawner: S,
    running: Vec<Running<S::Process>>,
}

impl<S: Spawner + Default> Default for Launches<S> {
    fn default() -> Self {
        Self::new(S::default())
    }
}

impl<S: Spawner> Launches<S> {
    pub const fn new(spawner: S) -> Self {
        Self {
            spawner,
            running: Vec::new(),
        }
    }

    /// Starts the child and owns it from here on.
    ///
    /// # Errors
    /// Too many children are pending, or the program cannot start.
    pub fn start(&mut self, launch: &Launch, now: Instant) -> Result<(), String> {
        if self.running.len() >= CAPACITY {
            return Err("too many pending launches".into());
        }
        let process = self.spawner.spawn(launch).map_err(|error| error.to_string())?;
        self.running.push(Running {
            process,
            label: launch.label.clone(),
            app_id: launch.app_id.clone(),
            started: now,
        });
        Ok(())
    }

    #[must_use]
    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// Reaps ended children and reports each unsuccessful end once.
    pub fn poll(&mut self, now: Instant) -> Vec<Failure> {
        let mut failures = Vec::new();
        self.running.retain_mut(|child| {
            let exit = match child.process.try_wait() {
                Ok(None) => return true,
                Ok(Some(exit)) => exit,
                Err(error) => {
                    tracing::warn!(%error, "launched child wait failed");
                    child.process.terminate();
                    Exit::Crashed
                }
            };
            if exit != Exit::Success {
                failures.push(Failure {
                    label: child.label.clone(),
                    app_id: child.app_id.clone(),
                    code: match exit {
                        Exit::Code(code) => Some(code),
                        Exit::Success | Exit::Crashed => None,
                    },
                    early: now.saturating_duration_since(child.started) < EARLY_EXIT,
                });
            }
            false
        });
        failures
    }
}

impl<S: Spawner> Drop for Launches<S> {
    fn drop(&mut self) {
        for child in &mut self.running {
            child.process.terminate();
        }
    }
}

/// A scripted spawner for tests: each launch ends as the script says.
#[derive(Debug, Default)]
pub struct FakeSpawner {
    pub started: Vec<Launch>,
    pub refuse: bool,
    pub next_exit: Option<Exit>,
}

/// A fake child: it ends with its scripted exit at the first poll, or never.
#[derive(Debug)]
pub struct FakeProcess(Option<Exit>);

impl Process for FakeProcess {
    fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        Ok(self.0)
    }

    fn terminate(&mut self) {}
}

impl Spawner for FakeSpawner {
    type Process = FakeProcess;

    fn spawn(&mut self, launch: &Launch) -> io::Result<FakeProcess> {
        if self.refuse {
            return Err(io::Error::new(io::ErrorKind::NotFound, "program not found"));
        }
        self.started.push(launch.clone());
        Ok(FakeProcess(self.next_exit))
    }
}
