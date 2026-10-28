//! Configured system menu commands, launched without a shell.

use std::{collections::BTreeMap, io, path::PathBuf, process::Stdio};
use tehuti_core::menu::Command;

const COMMANDS: [&str; 5] = ["log_out", "plasma", "reboot", "shut_down", "task_monitor"];

#[derive(Debug, Default)]
pub struct Config {
    commands: BTreeMap<String, Vec<String>>,
    notices: Vec<String>,
    entries: Vec<(String, Vec<String>, i64)>,
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
        config.entries.sort_by_key(|entry| entry.2);
        Ok(config)
    }

    #[must_use]
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    #[must_use]
    pub fn labels(&self) -> Vec<String> {
        self.entries.iter().map(|entry| entry.0.clone()).collect()
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
        let configured = match command {
            Command::Custom(index) => self.entries.get(index).map(|entry| &entry.1),
            Command::LogOut
            | Command::Restart
            | Command::Plasma
            | Command::Reboot
            | Command::ShutDown
            | Command::TaskMonitor
            | Command::Settings => self.commands.get(command.key()),
        };
        let (program, args) = configured
            .and_then(|argv| argv.split_first())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "menu command is not configured"))?;
        spawn(program, args)
    }
}

fn parse_entry(value: &toml::Value) -> Result<(String, Vec<String>, i64), String> {
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
    Ok((label.into(), argv, order))
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

/// Starts a command from the validated menu config.
///
/// # Errors
/// The command is disabled or could not be started.
pub fn execute(config: &Config, command: Command, socket: &std::ffi::OsStr) -> io::Result<()> {
    config.run(command, |program, args| {
        let mut child = crate::client_env::command(std::ffi::OsStr::new(program), socket)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let _reaper = std::thread::spawn(move || child.wait());
        Ok(())
    })
}
