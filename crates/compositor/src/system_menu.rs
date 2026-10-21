//! Configured system menu commands, launched without a shell.

use std::{collections::BTreeMap, io, path::PathBuf, process::Stdio};
use tehuti_core::menu::Command;

#[derive(Debug, Default)]
pub struct Config {
    commands: BTreeMap<String, Vec<String>>,
}

impl Config {
    /// Parses argument lists. Empty lists disable an entry.
    ///
    /// # Errors
    /// Unknown entries, non-string arguments and invalid program names are rejected.
    pub fn parse(text: &str) -> io::Result<Self> {
        let commands: BTreeMap<String, Vec<String>> =
            serde_json::from_str(text).map_err(io::Error::other)?;
        for (name, argv) in &commands {
            if !["log_out", "plasma", "reboot", "shut_down", "task_monitor"].contains(&name.as_str())
                || argv.first().is_some_and(String::is_empty)
                || argv.iter().any(|arg| arg.contains('\0'))
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid menu command"));
            }
        }
        Ok(Self { commands })
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
        let (program, args) = self.commands.get(command.key()).and_then(|argv| argv.split_first())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "menu command is not configured"))?;
        spawn(program, args)
    }
}

fn config_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("TEHUTI_MENU_CONFIG") {
        return Ok(path.into());
    }
    let directory = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
    }).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))?;
    Ok(directory.join("tehuti/menu.json"))
}

/// Loads the menu config and starts the selected command.
///
/// # Errors
/// The configuration could not be read or the command could not be started.
pub fn execute(command: Command) -> io::Result<()> {
    let config = Config::parse(&std::fs::read_to_string(config_path()?)?)?;
    config.run(command, |program, args| {
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let _reaper = std::thread::spawn(move || child.wait());
        Ok(())
    })
}
