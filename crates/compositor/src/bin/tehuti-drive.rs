//! Command-line client for a selected nested test session.
use core::{error::Error, time::Duration};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};
use tehuti::driver::{Request, request};

const HELP: &str = "tehuti-drive --socket PATH COMMAND\n\
start CONFIG [--headless] [--size WIDTHxHEIGHT] [-- CLIENT ARGS...]  Start an isolated test session\n\
stop                         Stop the selected test session\n\
press BUTTON                 Tap a pad button (A/B/X/Y or snake_case name)\n\
hold BUTTON                  Keep a pad button pressed until release\n\
release BUTTON               Release a pad button\n\
stick LX LY RX RY            Set sticks in thousandths, -1000..1000\n\
key KEY                      Tap a protocol key name\n\
text TEXT                    Type text into the focused client\n\
say TEXT                     Deliver a final transcript\n\
scene --json                 Inspect windows, focus, launcher, bar and notices\n\
shot PATH                    Save a whole-output PNG\n\
wait-for POINTER JSON [MS]    Wait for a JSON pointer to equal a value (default 5000 ms)\n\
clock advance MS             Advance the manual test clock\n\
--help                       Show this help\n\
Every command returns JSON. No default socket: select a test session explicitly.\n\
start requires a fresh, private runtime directory containing PATH.\n";

fn next(arguments: &mut impl Iterator<Item = String>) -> Result<String, Box<dyn Error>> {
    arguments
        .next()
        .ok_or_else(|| "missing argument; see --help".into())
}

fn main() -> Result<ExitCode, Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let first = next(&mut arguments)?;
    if first == "--help" || first == "-h" {
        write!(std::io::stdout().lock(), "{HELP}")?;
        return Ok(ExitCode::SUCCESS);
    }
    if first != "--socket" {
        return Err("--socket PATH is required".into());
    }
    let socket = PathBuf::from(next(&mut arguments)?);
    let command = next(&mut arguments)?;
    let value = match command.as_str() {
        "start" => start(&socket, arguments)?,
        "wait-for" => {
            let pointer = next(&mut arguments)?;
            if !pointer.starts_with('/') {
                return Err("condition must be a JSON pointer".into());
            }
            let expected: Value = serde_json::from_str(&next(&mut arguments)?)?;
            let timeout = arguments.next().map_or(Ok(5000), |value| value.parse::<u64>())?;
            if arguments.next().is_some() {
                return Err("unexpected argument".into());
            }
            wait(&socket, &pointer, &expected, Duration::from_millis(timeout))?
        }
        _ => {
            let action = parse_action(&command, &mut arguments)?;
            if arguments.next().is_some() {
                return Err("unexpected argument".into());
            }
            request(&socket, &action)?
        }
    };
    writeln!(std::io::stdout().lock(), "{value}")?;
    Ok(ExitCode::SUCCESS)
}

fn parse_action(
    command: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<Request, Box<dyn Error>> {
    let action = match command {
        "scene" => {
            if next(arguments)? != "--json" {
                return Err("use scene --json".into());
            }
            Request::Scene
        }
        "stop" => Request::Stop,
        "press" | "hold" | "release" => {
            let button = serde_json::from_value(json!(next(arguments)?))?;
            match command {
                "press" => Request::Press { button },
                "hold" => Request::Hold { button },
                _ => Request::Release { button },
            }
        }
        "stick" => Request::Stick {
            left: (next(arguments)?.parse()?, next(arguments)?.parse()?),
            right: (next(arguments)?.parse()?, next(arguments)?.parse()?),
        },
        "key" => Request::Key {
            key: serde_json::from_value(json!(next(arguments)?))?,
        },
        "text" => Request::Text {
            text: next(arguments)?,
        },
        "say" => Request::Say {
            text: next(arguments)?,
        },
        "shot" => Request::Shot {
            path: next(arguments)?.into(),
        },
        "clock" => {
            if next(arguments)? != "advance" {
                return Err("use clock advance MS".into());
            }
            Request::Advance {
                milliseconds: next(arguments)?.parse()?,
            }
        }
        _ => return Err("unknown command; see --help".into()),
    };
    Ok(action)
}

fn wait(socket: &Path, pointer: &str, expected: &Value, timeout: Duration) -> Result<Value, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        let scene = request(socket, &Request::Scene)?;
        if scene.pointer(pointer) == Some(expected) {
            return Ok(scene);
        }
        if started.elapsed() >= timeout {
            return Err(format!("wait timed out: {pointer} != {expected}").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn start(socket: &Path, mut arguments: impl Iterator<Item = String>) -> Result<Value, Box<dyn Error>> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let config = std::fs::canonicalize(next(&mut arguments)?)?;
    let runtime = socket.parent().ok_or("socket must have a runtime directory")?;
    if socket.file_name().and_then(|name| name.to_str()) != Some("tehuti-drive.sock") {
        return Err("socket filename must be tehuti-drive.sock".into());
    }
    std::fs::DirBuilder::new().mode(0o700).create(runtime)?;
    if std::fs::metadata(runtime)?.permissions().mode() & 0o077 != 0 {
        return Err("runtime directory must be private".into());
    }
    let runtime = std::fs::canonicalize(runtime)?;
    let selected_socket = runtime.join("tehuti-drive.sock");
    let executable = std::env::current_exe()?.with_file_name("tehuti");
    let mut command = std::process::Command::new(executable);
    let mut headless = false;
    let mut size = None;
    let mut client = vec![];
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--headless" => headless = true,
            "--size" => size = Some(next(&mut arguments)?),
            "--" => {
                client.extend(arguments);
                break;
            }
            _ => return Err("unknown start option".into()),
        }
    }
    if headless {
        let _command = command.arg("--test-driver");
    } else if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return Err("nested start requires a host display; use --headless".into());
    }
    // Preserve the host display connection while isolating child sockets.
    if let Some(display) = std::env::var_os("WAYLAND_DISPLAY") {
        let display = PathBuf::from(display);
        if !display.is_absolute() {
            let host_runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("host runtime unavailable")?;
            let _command = command.env("WAYLAND_DISPLAY", PathBuf::from(host_runtime).join(display));
        }
    }
    if let Some(size) = size {
        let _command = command.env("TEHUTI_TEST_SIZE", size);
    }
    let log = std::fs::File::create(runtime.join("session.log"))?;
    let mut child = command
        .arg(config)
        .args(client)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("XDG_CONFIG_HOME", runtime.join("config"))
        .env("XDG_DATA_HOME", runtime.join("data"))
        .env("XDG_STATE_HOME", runtime.join("state"))
        .env_remove("TEHUTI_SAY")
        .env_remove("TEHUTI_SPEECH_SIDECAR")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    await_start(&mut child, &selected_socket)
}

fn await_start(child: &mut std::process::Child, socket: &Path) -> Result<Value, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        if let Ok(scene) = request(socket, &Request::Scene) {
            return Ok(json!({"pid": child.id(), "socket": socket, "scene": scene}));
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("test session exited: {status}; see session.log").into());
        }
        if started.elapsed() > Duration::from_secs(15) {
            let _killed = child.kill();
            let _waited = child.wait();
            return Err("test session did not become ready; see session.log".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
