//! Configured system menu commands, launched without a shell.

use core::time::Duration;
use std::{collections::BTreeMap, io, path::PathBuf, process::Stdio, sync::mpsc::Sender};
use tehuti_core::menu::Command;

const COMMANDS: [&str; 5] = ["log_out", "plasma", "reboot", "shut_down", "task_monitor"];

static TASK_MONITOR: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| vec!["plasma-systemmonitor".to_owned()]);

/// How long a started menu command is watched for a failing exit.
const EXIT_WATCH: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Entry {
    label: String,
    argv: Vec<String>,
    order: i64,
}

#[derive(Debug, Default)]
pub struct Config {
    commands: BTreeMap<String, Vec<String>>,
    notices: Vec<String>,
    entries: Vec<Entry>,
}

impl Config {
    /// Parses argument lists. Empty lists disable an entry.
    ///
    /// # Errors
    /// Invalid TOML is rejected; invalid entries are skipped and returned as notices.
    pub fn parse(text: &str) -> io::Result<Self> {
        let values: toml::Table = toml::from_str(text).map_err(io::Error::other)?;
        let mut config = Self::default();
        for (name, value) in values {
            if name == "entry" {
                if let Some(entries) = value.as_array() {
                    for (index, entry) in entries.iter().enumerate() {
                        match parse_entry(entry) {
                            Ok(entry) => config.entries.push(entry),
                            Err(error) => config.notices.push(format!("menu entry[{index}]: {error}")),
                        }
                    }
                } else {
                    config
                        .notices
                        .push("menu entry: expected [[entry]] tables".into());
                }
                continue;
            }
            let result = value.try_into::<Vec<String>>().map_err(|error| error.to_string());
            let result = result.and_then(|argv| {
                if COMMANDS.contains(&name.as_str()) {
                    validate_argv(&argv, true).map(|()| argv)
                } else {
                    Err("unknown menu entry".into())
                }
            });
            match result {
                Ok(argv) => {
                    let _previous = config.commands.insert(name, argv);
                }
                Err(error) => config.notices.push(format!("menu entry {name:?}: {error}")),
            }
        }
        config.entries.sort_by_key(|entry| entry.order);
        Ok(config)
    }

    #[must_use]
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    #[must_use]
    pub fn labels(&self) -> Vec<String> {
        self.entries.iter().map(|entry| entry.label.clone()).collect()
    }

    #[must_use]
    pub fn session_exit_status(&self, command: Command) -> Option<u8> {
        if self.commands.contains_key(command.key()) {
            return None;
        }
        match command {
            Command::LogOut => Some(21),
            Command::Plasma => Some(20),
            Command::Restart
            | Command::Reboot
            | Command::ShutDown
            | Command::TaskMonitor
            | Command::StartRecording
            | Command::StopRecording
            | Command::ToggleRecording
            | Command::Screenshot
            | Command::Settings
            | Command::Custom(_) => None,
        }
    }

    /// Built-in menu commands with nothing to run: no configured argv and no default.
    #[must_use]
    pub fn hidden(&self) -> Vec<Command> {
        tehuti_core::menu::ITEMS
            .into_iter()
            .filter(|command| {
                matches!(
                    command,
                    Command::LogOut
                        | Command::Plasma
                        | Command::Reboot
                        | Command::ShutDown
                        | Command::TaskMonitor
                ) && self.session_exit_status(*command).is_none()
                    && self.argv(*command).is_none()
            })
            .collect()
    }

    /// The name a notice uses for `command`.
    #[must_use]
    pub fn label(&self, command: Command) -> String {
        match command {
            Command::Custom(index) => self
                .entries
                .get(index)
                .map_or_else(|| "MENU ENTRY".into(), |entry| entry.label.clone()),
            Command::LogOut
            | Command::Restart
            | Command::Plasma
            | Command::Reboot
            | Command::ShutDown
            | Command::TaskMonitor
            | Command::StartRecording
            | Command::StopRecording
            | Command::ToggleRecording
            | Command::Screenshot
            | Command::Settings => command.label().into(),
        }
    }

    fn argv(&self, command: Command) -> Option<(&String, &[String])> {
        let configured = match command {
            Command::Custom(index) => self.entries.get(index).map(|entry| &entry.argv),
            Command::LogOut
            | Command::Restart
            | Command::Plasma
            | Command::Reboot
            | Command::ShutDown
            | Command::TaskMonitor
            | Command::StartRecording
            | Command::StopRecording
            | Command::ToggleRecording
            | Command::Screenshot
            | Command::Settings => self.commands.get(command.key()),
        };
        match configured {
            Some(argv) => argv.split_first(),
            None if command == Command::TaskMonitor => TASK_MONITOR.split_first(),
            None => None,
        }
    }

    /// Passes one configured command to the executor.
    ///
    /// # Errors
    /// The command is disabled or the executor failed.
    pub fn run(
        &self,
        command: Command,
        spawn: impl FnOnce(&str, &[String]) -> io::Result<()>,
    ) -> io::Result<()> {
        let (program, args) = self
            .argv(command)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "menu command is not configured"))?;
        spawn(program, args).map_err(|error| io::Error::new(error.kind(), format!("{program}: {error}")))
    }
}

fn parse_entry(value: &toml::Value) -> Result<Entry, String> {
    let table = value.as_table().ok_or("expected a table")?;
    if table
        .keys()
        .any(|key| !["label", "exec", "order"].contains(&key.as_str()))
    {
        return Err("unknown field, expected label, exec and order".into());
    }
    let label = table
        .get("label")
        .and_then(toml::Value::as_str)
        .ok_or("label must be a string")?;
    if label.trim().is_empty() || label.chars().any(char::is_control) {
        return Err("label must be nonempty and contain no control characters".into());
    }
    let argv = table
        .get("exec")
        .ok_or("exec is required")?
        .clone()
        .try_into::<Vec<String>>()
        .map_err(|error| error.to_string())?;
    validate_argv(&argv, false)?;
    let order = table
        .get("order")
        .and_then(toml::Value::as_integer)
        .ok_or("order must be an integer")?;
    Ok(Entry {
        label: label.into(),
        argv,
        order,
    })
}

/// Loads and validates menu commands before the menu can be opened.
///
/// # Errors
/// The file could not be read or contains invalid TOML.
pub fn load() -> io::Result<Config> {
    match std::fs::read_to_string(config_path()?) {
        Ok(text) => Config::parse(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(error) => Err(error),
    }
}

fn validate_argv(argv: &[String], allow_empty: bool) -> Result<(), String> {
    if argv.is_empty() && !allow_empty {
        Err("exec must contain a program".into())
    } else if argv.first().is_some_and(|program| program.trim().is_empty()) {
        Err("empty program name".into())
    } else if argv.iter().any(|arg| arg.contains('\0')) {
        Err("argument contains NUL".into())
    } else {
        Ok(())
    }
}

fn config_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("TEHUTI_MENU_CONFIG") {
        return Ok(path.into());
    }
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))?;
    Ok(directory.join("tehuti/menu.toml"))
}

/// Starts a command from the validated menu config. A non-zero exit within a few seconds
/// sends one line naming the entry to `failures`.
///
/// # Errors
/// The command is disabled or could not be started.
pub fn execute(
    config: &Config,
    command: Command,
    socket: &std::ffi::OsStr,
    failures: Sender<String>,
) -> io::Result<()> {
    let label = config.label(command);
    config.run(command, |program, args| {
        let mut child = crate::client_env::command(std::ffi::OsStr::new(program), socket)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let program = program.to_owned();
        let _reaper = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            while started.elapsed() < EXIT_WATCH {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            let _sent = failures.send(format!("{label} FAILED: {program} {status}"));
                        }
                        return;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => break,
                }
            }
            let _status = child.wait();
        });
        Ok(())
    })
}
