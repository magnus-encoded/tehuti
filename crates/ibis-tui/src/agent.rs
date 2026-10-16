//! JSON-lines access to the terminal's command state.
use ibis_commands::{Button, Effect, Input, State, step};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// Command state shared by terminal and socket inputs.
#[derive(Debug)]
pub struct Session {
    state: State,
    initial: State,
    cwd: PathBuf,
}

impl Session {
    /// Keep the initial environment for resets.
    #[must_use]
    pub fn new(state: State, cwd: PathBuf) -> Self {
        Self {
            initial: state.clone(),
            state,
            cwd,
        }
    }

    /// Current builder and history.
    #[must_use]
    pub const fn state(&self) -> &State {
        &self.state
    }

    /// Apply an input and complete any command through the terminal runner.
    pub fn input(&mut self, input: Input) {
        let effects;
        (self.state, effects) = step(self.state.clone(), input);
        for Effect::Run(invocation) in effects {
            let (exit, output) =
                crate::run(&invocation, &self.cwd).unwrap_or_else(|error| (126, error.to_string()));
            (self.state, _) = step(
                self.state.clone(),
                Input::Completed {
                    invocation,
                    exit,
                    output,
                },
            );
        }
    }

    fn request(&mut self, line: &[u8]) -> Result<String, String> {
        let args: Vec<String> = serde_json::from_slice(line).map_err(|error| error.to_string())?;
        let (verb, rest) = args.split_first().ok_or("expected a verb")?;
        let inputs = match verb.as_str() {
            "screen" if rest.is_empty() => Vec::new(),
            "reset" if rest.is_empty() => {
                self.state = self.initial.clone();
                Vec::new()
            }
            "press" if !rest.is_empty() => rest
                .iter()
                .map(|button| {
                    Button::parse(button)
                        .map(Input::Press)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
            "say" if !rest.is_empty() => vec![Input::Say(rest.join(" "))],
            "type" if !rest.is_empty() => vec![Input::Type(rest.join(" "))],
            _ => return Err("expected screen | press BUTTON... | say TEXT | type TEXT | reset".into()),
        };
        for input in inputs {
            self.input(input);
        }
        Ok(self.state.screen().to_string())
    }
}

/// Resolve the override or the runtime directory's default socket.
///
/// # Errors
/// Returns an error when neither environment variable is set.
pub fn socket_path(
    socket: Option<std::ffi::OsString>,
    runtime: Option<std::ffi::OsString>,
) -> io::Result<PathBuf> {
    socket
        .map(PathBuf::from)
        .or_else(|| runtime.map(|path| PathBuf::from(path).join("ibis-tui.sock")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "set XDG_RUNTIME_DIR or IBIS_TUI_SOCKET"))
}

/// Nonblocking clients cannot hold up terminal input while sending a line.
#[derive(Debug)]
pub struct Socket {
    listener: UnixListener,
    clients: Vec<Client>,
    path: PathBuf,
}

#[derive(Debug)]
struct Client {
    stream: UnixStream,
    input: Vec<u8>,
    output: Vec<u8>,
}

impl Socket {
    /// Replace a stale socket and listen for local agents.
    ///
    /// # Errors
    /// Returns filesystem and socket setup errors, including an active listener.
    pub fn bind(path: &Path) -> io::Result<Self> {
        if path.try_exists()? {
            if !std::fs::symlink_metadata(path)?.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "agent path is not a socket",
                ));
            }
            match UnixStream::connect(path) {
                Ok(_) => return Err(io::Error::new(io::ErrorKind::AddrInUse, "agent socket is active")),
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => std::fs::remove_file(path)?,
                Err(error) => return Err(error),
            }
        }
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            clients: Vec::new(),
            path: path.to_owned(),
        })
    }

    /// Process available lines, isolating malformed or disconnected clients.
    ///
    /// # Errors
    /// Returns errors accepting a connection; individual client errors close that client.
    pub fn poll(&mut self, session: &mut Session) -> io::Result<()> {
        for _ in 0..16 {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true)?;
                    if self.clients.len() < 16 {
                        self.clients.push(Client {
                            stream,
                            input: Vec::new(),
                            output: Vec::new(),
                        });
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        self.clients.retain_mut(|client| client.poll(session).is_ok());
        Ok(())
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _removed = std::fs::remove_file(&self.path);
    }
}

impl Client {
    fn poll(&mut self, session: &mut Session) -> io::Result<()> {
        let mut bytes = [0; 4096];
        if self.output.is_empty() {
            if !self.input.contains(&b'\n') {
                match self.stream.read(&mut bytes) {
                    Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    Ok(count) => self
                        .input
                        .extend_from_slice(bytes.get(..count).unwrap_or_default()),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            if self.input.len() > 65536 {
                return Err(io::ErrorKind::InvalidData.into());
            }
            if let Some(end) = self.input.iter().position(|byte| *byte == b'\n') {
                let line: Vec<_> = self.input.drain(..=end).collect();
                let reply = match session.request(&line) {
                    Ok(screen) => serde_json::json!({ "screen": screen }),
                    Err(error) => serde_json::json!({ "error": error }),
                };
                self.output = serde_json::to_vec(&reply)?;
                self.output.push(b'\n');
            }
        }
        if !self.output.is_empty() {
            match self.stream.write(&self.output) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => {
                    let _sent: Vec<_> = self.output.drain(..count).collect();
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::socket_path;
    use std::path::PathBuf;

    #[test]
    fn socket_override_precedes_runtime_directory() -> Result<(), std::io::Error> {
        assert_eq!(
            socket_path(None, Some("/runtime".into()))?,
            PathBuf::from("/runtime/ibis-tui.sock")
        );
        assert_eq!(
            socket_path(Some("/override".into()), Some("/runtime".into()))?,
            PathBuf::from("/override")
        );
        assert_eq!(
            socket_path(Some("/override".into()), None)?,
            PathBuf::from("/override")
        );
        assert!(socket_path(None, None).is_err());
        Ok(())
    }
}
