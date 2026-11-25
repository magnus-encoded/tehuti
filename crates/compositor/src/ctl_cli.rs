//! Command-line boundary for session control.
use crate::ctl::{Command, send, socket_path};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

pub const USAGE: &str = "usage: tehuti ctl [--json] [--socket PATH] COMMAND
status | windows | workspaces | outputs | events
layout ACTION | focus ID | close [ID] | launch APP | launch --command PROGRAM [ARGS...]
notice TEXT | display mode WIDTHxHEIGHT[@HZ] | display mode --restore | display scale N
shot [output|window] [PATH] | context list|active|reload|check [PATH] | plugin list | session restart
Every command accepts --help and --json. Events always emit NDJSON.
";

/// # Errors
/// Reports invalid arguments, unavailable commands and session errors.
pub fn run(arguments: Vec<OsString>, output: &mut impl Write) -> Result<(), Box<dyn core::error::Error>> {
    let json = arguments.iter().any(|argument| argument == "--json");
    let result = run_inner(arguments, output);
    if let Err(error) = &result
        && json
    {
        writeln!(
            output,
            "{}",
            serde_json::json!({"v": 1, "error": error.to_string()})
        )?;
    }
    result
}

fn run_inner(arguments: Vec<OsString>, output: &mut impl Write) -> Result<(), Box<dyn core::error::Error>> {
    let mut words = arguments
        .into_iter()
        .map(|word| word.into_string().map_err(|_| "ctl requires UTF-8 arguments"))
        .collect::<Result<Vec<_>, _>>()?;
    if words.iter().any(|word| word == "--help" || word == "-h") || words.is_empty() {
        output.write_all(USAGE.as_bytes())?;
        return Ok(());
    }
    words.retain(|word| word != "--json");
    let path = if let Some(index) = words.iter().position(|word| word == "--socket") {
        let _flag = words.remove(index);
        if index >= words.len() {
            return Err("--socket requires a path".into());
        }
        PathBuf::from(words.remove(index))
    } else {
        socket_path()?
    };
    let command = parse(&words)?;
    send(&path, command, output)?;
    Ok(())
}

/// # Errors
/// Rejects malformed commands before connecting to the session.
pub fn parse(words: &[String]) -> Result<Command, String> {
    let words: Vec<_> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["status"] => Ok(Command::Status),
        ["windows"] => Ok(Command::Windows),
        ["workspaces"] => Ok(Command::Workspaces),
        ["outputs"] => Ok(Command::Outputs),
        ["events"] => Ok(Command::Events),
        ["layout", action] => serde_json::from_value(serde_json::json!(action))
            .map(|action| Command::Layout { action })
            .map_err(|error| error.to_string()),
        ["focus", identity] => identity
            .parse()
            .map(|identity| Command::Focus { identity })
            .map_err(|_| "focus requires a window id".into()),
        ["close"] => Ok(Command::Close { identity: None }),
        ["close", identity] => identity
            .parse()
            .map(|identity| Command::Close {
                identity: Some(identity),
            })
            .map_err(|_| "close requires a window id".into()),
        ["launch", "--command", program, arguments @ ..] if !program.is_empty() => {
            Ok(Command::LaunchCommand {
                program: (*program).into(),
                arguments: arguments.iter().map(|word| (*word).to_owned()).collect(),
            })
        }
        ["launch", app] if !app.starts_with('-') => Ok(Command::Launch { app: (*app).into() }),
        ["notice", text] => Ok(Command::Notice { text: (*text).into() }),
        ["display", "mode", mode] if crate::output_mode::Request::parse(mode).is_some() => {
            Ok(Command::DisplayMode { mode: (*mode).into() })
        }
        ["display", "scale", scale] => scale
            .parse::<u32>()
            .ok()
            .filter(|scale| (1..=8).contains(scale))
            .map(|scale| Command::DisplayScale { scale })
            .ok_or_else(|| "scale must be an integer from 1 to 8".into()),
        ["shot"] | ["shot", "output"] => Ok(Command::Shot {
            path: default_shot()?,
        }),
        ["shot", "output", path] | ["shot", path] if *path != "window" => {
            Ok(Command::Shot { path: (*path).into() })
        }
        ["shot", "window"] => Ok(Command::ShotWindow {
            path: default_shot()?,
        }),
        ["shot", "window", path] => Ok(Command::ShotWindow { path: (*path).into() }),
        ["context", "list"] => Ok(Command::ContextList),
        ["context", "active"] => Ok(Command::ContextActive),
        ["context", "reload"] => Ok(Command::ContextReload),
        ["context", "check"] => Ok(Command::ContextCheck { path: None }),
        ["context", "check", path] => Ok(Command::ContextCheck {
            path: Some((*path).into()),
        }),
        ["plugin", "list"] => Ok(Command::PluginList),
        ["session", "restart"] => Ok(Command::SessionRestart),
        [group, ..] => Err(not_yet(group)),
        [] => Err(USAGE.into()),
    }
}

fn default_shot() -> Result<PathBuf, String> {
    std::env::current_dir()
        .map(|path| path.join("tehuti-shot.png"))
        .map_err(|error| error.to_string())
}

fn not_yet(group: &str) -> String {
    let issue = match group {
        "volume" | "audio" => 918,
        "record" => 920,
        "brightness" => 922,
        "update" => 923,
        "help-overlay" => 924,
        "toggle" | "toggles" | "bar" => 925,
        "wallpaper" => 928,
        "settings" => 928,
        "session" => 341,
        _ => 928,
    };
    format!("invalid or not yet available ctl command {group}; see #{issue}")
}

/// # Errors
/// Writes help or one command's result to stdout.
pub fn stdout(arguments: Vec<OsString>) -> Result<(), Box<dyn core::error::Error>> {
    run(arguments, &mut io::stdout().lock())
}
