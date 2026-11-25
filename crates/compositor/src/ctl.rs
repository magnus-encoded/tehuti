//! Versioned session control, separate from synthetic input.
use core::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Instant,
};

const LIMIT: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    #[serde(rename = "v")]
    pub version: u8,
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    Windows,
    Workspaces,
    Outputs,
    Events,
    Layout {
        action: tehuti_protocol::LayoutAction,
    },
    Focus {
        #[serde(rename = "id")]
        identity: u64,
    },
    Close {
        #[serde(rename = "id")]
        identity: Option<u64>,
    },
    Launch {
        app: String,
    },
    LaunchCommand {
        program: String,
        arguments: Vec<String>,
    },
    Notice {
        text: String,
    },
    DisplayMode {
        mode: String,
    },
    DisplayScale {
        scale: u32,
    },
    Shot {
        path: PathBuf,
    },
    ShotWindow {
        path: PathBuf,
    },
    ContextList,
    ContextActive,
    ContextReload,
    ContextCheck {
        path: Option<PathBuf>,
    },
    PluginList,
    SessionRestart,
}

/// # Errors
/// Requires the session's runtime directory.
pub fn socket_path() -> io::Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(|directory| PathBuf::from(directory).join("tehuti-control/ctl"))
        .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))
}

#[derive(Debug)]
struct Peer {
    stream: UnixStream,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    started: Instant,
    events: bool,
    last: Option<Value>,
    sequence: u64,
}

#[derive(Debug)]
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    peers: Vec<Peer>,
}

impl Server {
    /// # Errors
    /// Refuses nonprivate directories and existing live sockets.
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("missing control directory"))?;
        if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(directory)
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(error);
        }
        let metadata = std::fs::symlink_metadata(directory)?;
        if !metadata.is_dir()
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != rustix::process::getuid().as_raw()
        {
            return Err(io::Error::other(
                "control directory must be private and owned by the session user",
            ));
        }
        if let Ok(socket_metadata) = std::fs::symlink_metadata(&path) {
            if !socket_metadata.file_type().is_socket() {
                return Err(io::Error::other("control path is not a socket"));
            }
            match UnixStream::connect(&path) {
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(&path)?;
                }
                _ => return Err(io::Error::other("another session owns the control socket")),
            }
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            peers: vec![],
        })
    }

    /// Services bounded requests; slow subscribers are disconnected.
    pub fn poll(&mut self, mut execute: impl FnMut(Command) -> Result<Value, String>) {
        for _ in 0..16 {
            if self.peers.len() >= 32 {
                break;
            }
            let Ok((stream, _)) = self.listener.accept() else {
                break;
            };
            if !same_user(&stream) || stream.set_nonblocking(true).is_err() {
                continue;
            }
            self.peers.push(Peer {
                stream,
                input: vec![],
                output: vec![],
                written: 0,
                started: Instant::now(),
                events: false,
                last: None,
                sequence: 0,
            });
        }
        self.peers.retain_mut(|peer| service(peer, &mut execute));
    }
}

fn same_user(stream: &UnixStream) -> bool {
    rustix::net::sockopt::socket_peercred(stream)
        .is_ok_and(|credentials| credentials.uid == rustix::process::getuid())
}

#[expect(
    clippy::too_many_lines,
    reason = "bounded framing, execution and partial writes share one peer lifetime"
)]
fn service(peer: &mut Peer, execute: &mut impl FnMut(Command) -> Result<Value, String>) -> bool {
    if peer.started.elapsed() > Duration::from_secs(5) && (!peer.events || !peer.output.is_empty()) {
        return false;
    }
    if peer.events {
        let mut probe = [0; 1];
        match peer.stream.read(&mut probe) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            _ => return false,
        }
    }
    if peer.output.is_empty() {
        if peer.events {
            let Ok(snapshot) = execute(Command::Events) else {
                return false;
            };
            if peer.last.as_ref() == Some(&snapshot) {
                return true;
            }
            peer.sequence = peer.sequence.saturating_add(1);
            peer.output = json!({"v": 1, "event": "state", "sequence": peer.sequence, "data": snapshot})
                .to_string()
                .into_bytes();
            peer.last = Some(snapshot);
        } else {
            let mut buffer = [0; 4096];
            match peer.stream.read(&mut buffer) {
                Ok(0) => return false,
                Ok(count) => peer
                    .input
                    .extend_from_slice(buffer.get(..count).unwrap_or_default()),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
                Err(_) => return false,
            }
            if peer.input.len() > LIMIT {
                return false;
            }
            let Some(end) = peer.input.iter().position(|byte| *byte == b'\n') else {
                return true;
            };
            let result = serde_json::from_slice::<Request>(peer.input.get(..end).unwrap_or_default())
                .map_err(|_| "invalid ctl request".to_owned())
                .and_then(|request| {
                    if request.version != 1 {
                        return Err("unsupported ctl version".into());
                    }
                    peer.events = matches!(request.command, Command::Events);
                    execute(request.command)
                });
            let reply = match result {
                Ok(value) if peer.events => {
                    peer.last = Some(value.clone());
                    json!({"v": 1, "event": "state", "sequence": 0, "data": value})
                }
                Ok(value) => json!({"v": 1, "ok": value}),
                Err(error) => {
                    peer.events = false;
                    json!({"v": 1, "error": error})
                }
            };
            peer.output = reply.to_string().into_bytes();
        }
        if peer.output.len() > 4_194_304 {
            return false;
        }
        peer.output.push(b'\n');
        peer.started = Instant::now();
    }
    match peer
        .stream
        .write(peer.output.get(peer.written..).unwrap_or_default())
    {
        Ok(0) => false,
        Ok(count) => {
            peer.written = peer.written.saturating_add(count);
            if peer.written < peer.output.len() {
                return true;
            }
            if !peer.events {
                return false;
            }
            peer.output.clear();
            peer.written = 0;
            true
        }
        Err(error) => error.kind() == io::ErrorKind::WouldBlock,
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _removed = std::fs::remove_file(&self.path);
    }
}

/// # Errors
/// Reports transport and remote errors. Events stream until the consumer closes stdout.
pub fn send(path: &Path, command: Command, output: &mut impl Write) -> io::Result<()> {
    let events = matches!(command, Command::Events);
    let mut stream = UnixStream::connect(path)?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    if !events {
        stream.set_read_timeout(Some(Duration::from_secs(6)))?;
    }
    serde_json::to_writer(&mut stream, &Request { version: 1, command })?;
    stream.write_all(b"\n")?;
    let mut reader = io::BufReader::new(stream);
    loop {
        let mut bytes = Vec::new();
        let count = Read::by_ref(&mut reader)
            .take(4_194_305)
            .read_until(b'\n', &mut bytes)?;
        if count == 0 || count > 4_194_304 || bytes.last() != Some(&b'\n') {
            return Err(io::Error::other("invalid ctl response"));
        }
        let value: Value = serde_json::from_slice(&bytes)?;
        if value.get("v").and_then(Value::as_u64) != Some(1) {
            return Err(io::Error::other("unsupported ctl response version"));
        }
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(io::Error::other(error.to_owned()));
        }
        output.write_all(&bytes)?;
        output.flush()?;
        if !events {
            return Ok(());
        }
    }
}
