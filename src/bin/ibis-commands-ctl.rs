use core::error::Error;
use core::time::Duration;
use ibis_commands::{Button, Effect, Environment, Input, ReadyInvocation, State, specs, step};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

type Result<T> = core::result::Result<T, Box<dyn Error>>;

fn names(root: &Path, relative: &Path, depth: usize, env: &mut Environment) {
    if depth > 8 || env.names.len() >= 100_000 {
        return;
    }
    let Ok(entries) = fs::read_dir(root.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = relative.join(entry.file_name());
        env.names.push(path.to_string_lossy().into_owned());
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            env.folders.push(path.to_string_lossy().into_owned());
            names(root, &path, depth.saturating_add(1), env);
        }
    }
}

fn environment(home: &Path) -> Environment {
    let mut env = Environment::default();
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            if let Ok(entries) = fs::read_dir(directory) {
                for entry in entries.flatten() {
                    if entry.metadata().is_ok_and(|metadata| {
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    }) {
                        env.programs
                            .push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    names(home, Path::new(""), 0, &mut env);
    env.programs.sort();
    env.programs.dedup();
    env.names.sort();
    env
}

fn run(invocation: &ReadyInvocation, home: &Path, state_path: &Path) -> Result<(i32, String)> {
    let capture_path = state_path.with_extension(format!("{}.output", std::process::id()));
    let capture = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&capture_path)?;
    let result = run_captured(invocation, home, &capture_path, capture);
    fs::remove_file(capture_path)?;
    result
}

fn run_captured(
    invocation: &ReadyInvocation,
    home: &Path,
    path: &Path,
    capture: File,
) -> Result<(i32, String)> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(invocation.shell_text())
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(capture.try_clone()?)
        .stderr(capture)
        .process_group(0)
        .spawn()?;
    let start = Instant::now();
    let code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code().unwrap_or(128);
        }
        if start.elapsed() >= Duration::from_secs(10) {
            let _status = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .status()?;
            child.kill().or_else(|error| {
                if error.kind() == std::io::ErrorKind::InvalidInput {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
            let _child_status = child.wait()?;
            break 124;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let bytes = fs::read(path)?;
    let tail = bytes.get(bytes.len().saturating_sub(4000)..).unwrap_or(&bytes);
    let mut output = String::from_utf8_lossy(tail).into_owned();
    if code == 124 {
        output.push_str("\n(timed out after 10 s)");
    }
    Ok((code, output))
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let home =
        fs::canonicalize(std::env::var_os("TB_HOME").map_or_else(|| PathBuf::from("."), PathBuf::from))?;
    let path =
        std::env::var_os("TB_STATE").map_or_else(|| PathBuf::from("ibis-commands-state.json"), PathBuf::from);
    let mut state = if args.first().is_some_and(|verb| verb == "reset") {
        State::new(home.to_string_lossy().into_owned(), specs()?)
    } else {
        match fs::read(&path) {
            Ok(bytes) => {
                let state: State = serde_json::from_slice(&bytes)?;
                if !state.valid() {
                    return Err("invalid builder state".into());
                }
                state
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                State::new(home.to_string_lossy().into_owned(), specs()?)
            }
            Err(error) => return Err(error.into()),
        }
    };
    let verb = args.first().map_or("screen", String::as_str);
    let inputs = inputs(&args, verb, &mut state, &home)?;
    let folder = PathBuf::from(state.cwd());
    (state, _) = step(state, Input::Environment(environment(&folder)));
    let log_path = std::env::var_os("TB_LOG")
        .map_or_else(|| PathBuf::from(format!("{}.log", path.display())), PathBuf::from);
    let mut log = OpenOptions::new().create(true).append(true).open(log_path)?;
    for input in inputs {
        let event = match &input {
            Input::Press(button) => serde_json::json!(["press", serde_json::to_value(button)?]),
            Input::Say(text) => serde_json::json!(["say", text]),
            Input::Type(text) => serde_json::json!(["type", text]),
            Input::Environment(_) | Input::Completed { .. } => return Err("unexpected host input".into()),
        };
        writeln!(log, "{event}")?;
        let effects;
        (state, effects) = step(state, input);
        for effect in effects {
            let Effect::Run(invocation) = effect else {
                continue;
            };
            let run_folder = PathBuf::from(state.cwd());
            let (exit, output) = run(&invocation, &run_folder, &path)?;
            (state, _) = step(
                state,
                Input::Completed {
                    invocation,
                    exit,
                    output,
                },
            );
        }
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec(&state)?)?;
    fs::rename(temporary, path)?;
    write!(std::io::stdout().lock(), "{}", state.screen())?;
    Ok(())
}

fn inputs(args: &[String], verb: &str, state: &mut State, home: &Path) -> Result<Vec<Input>> {
    let inputs = match verb {
        "screen" if args.len() <= 1 => Vec::new(),
        "reset" if args.len() == 1 => {
            *state = State::new(home.to_string_lossy().into_owned(), specs()?);
            Vec::new()
        }
        "press" if args.len() > 1 => args
            .iter()
            .skip(1)
            .map(|button| Button::parse(button).map(Input::Press))
            .collect::<core::result::Result<Vec<_>, _>>()?,
        "say" if args.len() > 1 => vec![Input::Say(
            args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
        )],
        "type" if args.len() > 1 => vec![Input::Type(
            args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
        )],
        _ => {
            return Err(
                "usage: ibis-commands-ctl screen | press BUTTON... | say TEXT | type TEXT | reset".into(),
            );
        }
    };
    Ok(inputs)
}
