//! Bounded, session-local control for nested test instances.
use crate::{backend::Backend, state::Compositor};
use core::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Instant,
};
use tehuti_protocol::{ButtonName, ButtonState, Event, KeyName, Text};

const LIMIT: usize = 65_536;

#[derive(Debug, Default)]
pub(crate) enum Clock {
    #[default]
    Live,
    Manual,
    Synthetic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Scene,
    Press { button: ButtonName },
    Hold { button: ButtonName },
    Release { button: ButtonName },
    Stick { left: (i16, i16), right: (i16, i16) },
    Key { key: KeyName },
    Text { text: String },
    Say { text: String },
    Advance { milliseconds: u64 },
    Shot { path: PathBuf },
    Stop,
}

#[derive(Debug)]
struct Peer {
    stream: UnixStream,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    started: Instant,
}

#[derive(Debug)]
pub struct Driver {
    listener: UnixListener,
    path: PathBuf,
    peers: Vec<Peer>,
    stopped: bool,
}

impl Driver {
    /// Creates a private socket, refusing every seat backend.
    /// # Errors
    /// Returns an error for a live backend or an unavailable socket path.
    pub fn bind(directory: &Path, backend: Backend) -> io::Result<Self> {
        if backend != Backend::Nested {
            return Err(io::Error::other(
                "driver requires a nested or headless test session",
            ));
        }
        let path = directory.join("tehuti-drive.sock");
        // Bind inside a private directory so the socket is never exposed before chmod.
        let private = directory.join("tehuti-drive");
        std::fs::DirBuilder::new().mode(0o700).create(&private)?;
        let bound = private.join("socket");
        let result = (|| {
            let listener = UnixListener::bind(&bound)?;
            std::fs::set_permissions(&bound, std::fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            if path.symlink_metadata().is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "driver socket already exists",
                ));
            }
            std::fs::rename(&bound, &path)?;
            Ok(Self {
                listener,
                path,
                peers: vec![],
                stopped: false,
            })
        })();
        let _removed = std::fs::remove_file(bound);
        let _directory_removed = std::fs::remove_dir(private);
        result
    }

    #[must_use]
    pub const fn stopped(&self) -> bool {
        self.stopped
    }

    /// Services bounded nonblocking requests on the compositor thread.
    pub fn poll(&mut self, compositor: &mut Compositor) {
        self.accept_peers();
        let mut peers = core::mem::take(&mut self.peers);
        peers.retain_mut(|peer| {
            if peer.started.elapsed() > Duration::from_secs(5) {
                return false;
            }
            if peer.output.is_empty() {
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
                    .map_err(|error| error.to_string())
                    .and_then(|request| {
                        self.execute(compositor, request)
                            .map_err(|error| error.to_string())
                    });
                let reply = match result {
                    Ok(value) => json!({"ok": value}),
                    Err(error) => json!({"error": error}),
                };
                peer.output = reply.to_string().into_bytes();
                peer.output.push(b'\n');
            }
            match peer
                .stream
                .write(peer.output.get(peer.written..).unwrap_or_default())
            {
                Ok(0) => false,
                Ok(count) => {
                    peer.written = peer.written.saturating_add(count);
                    peer.written < peer.output.len()
                }
                Err(error) => error.kind() == io::ErrorKind::WouldBlock,
            }
        });
        self.peers = peers;
    }

    fn accept_peers(&mut self) {
        for _ in 0..16 {
            let Ok((stream, _)) = self.listener.accept() else {
                break;
            };
            if self.peers.len() < 32 && stream.set_nonblocking(true).is_ok() {
                self.peers.push(Peer {
                    stream,
                    input: vec![],
                    output: vec![],
                    written: 0,
                    started: Instant::now(),
                });
            }
        }
    }

    /// Routes a validated request through compositor input seams.
    /// # Errors
    /// Rejects invalid input, approval interaction and failed captures.
    pub fn execute(
        &mut self,
        compositor: &mut Compositor,
        request: Request,
    ) -> Result<Value, Box<dyn core::error::Error>> {
        compositor.use_driver_clock();
        compositor.set_driver_input(true);
        let result = self.execute_input(compositor, request);
        compositor.set_driver_input(false);
        result
    }

    fn execute_input(
        &mut self,
        compositor: &mut Compositor,
        request: Request,
    ) -> Result<Value, Box<dyn core::error::Error>> {
        if !matches!(request, Request::Scene | Request::Shot { .. } | Request::Stop)
            && compositor.secret_confirmation_text().is_some()
        {
            return Err("synthetic input cannot answer an approval prompt".into());
        }
        match request {
            Request::Scene => return Ok(compositor.driver_scene()),
            Request::Press { button } => {
                for state in [ButtonState::Pressed, ButtonState::Released] {
                    compositor.handle(&Event::Button { name: button, state });
                }
            }
            Request::Hold { button } => compositor.handle(&Event::Button {
                name: button,
                state: ButtonState::Pressed,
            }),
            Request::Release { button } => compositor.handle(&Event::Button {
                name: button,
                state: ButtonState::Released,
            }),
            Request::Stick { left, right } => {
                if [left.0, left.1, right.0, right.1]
                    .iter()
                    .any(|value| !(-1000..=1000).contains(value))
                {
                    return Err("stick coordinates must be between -1000 and 1000".into());
                }
                compositor.sample_sticks(tehuti_core::dispatch::Sticks { left, right });
            }
            Request::Key { key } => {
                compositor.driver_action(&tehuti_protocol::Action::Key { key, mods: vec![] });
            }
            Request::Text { text } => compositor.driver_action(&tehuti_protocol::Action::Text {
                text: Text::new(text)?,
            }),
            Request::Say { text } => compositor.say(Text::new(text)?),
            Request::Advance { milliseconds } => {
                if milliseconds > 3_600_000 {
                    return Err("advance is limited to one hour per request".into());
                }
                compositor.handle(&Event::Tick {
                    elapsed: Duration::from_millis(milliseconds),
                });
            }
            Request::Shot { path } => crate::driver_shot::save(compositor, &path)?,
            Request::Stop => self.stopped = true,
        }
        Ok(json!({}))
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _removed = std::fs::remove_file(&self.path);
    }
}

use std::os::unix::fs::DirBuilderExt;

/// Sends one framed request to an explicitly selected test session.
/// # Errors
/// Returns connection, framing and remote errors.
pub fn request(path: &Path, request: &Request) -> Result<Value, Box<dyn core::error::Error>> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(6)))?;
    stream.set_write_timeout(Some(Duration::from_secs(6)))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let mut bytes = Vec::new();
    let _read = stream.take(4_194_304).read_to_end(&mut bytes)?;
    let value: Value = serde_json::from_slice(&bytes)?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_owned().into());
    }
    value
        .get("ok")
        .cloned()
        .ok_or_else(|| "invalid driver response".into())
}

/// Installs an output without acquiring any seat or display device.
/// # Errors
/// Rejects invalid output dimensions.
pub fn headless_output(
    display: &smithay::reexports::wayland_server::DisplayHandle,
    compositor: &mut Compositor,
) -> Result<(), Box<dyn core::error::Error>> {
    use smithay::{
        output::{Mode, Output, PhysicalProperties, Subpixel},
        utils::Transform,
    };
    let size = test_size()?;
    let mode = Mode {
        size: size.into(),
        refresh: 60_000,
    };
    let output = Output::new(
        "test".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Tehuti".into(),
            model: "Test".into(),
        },
    );
    let _global = output.create_global::<Compositor>(display);
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);
    compositor.space.map_output(&output, (0, 0));
    compositor.output = Some(output);
    compositor.fill_output();
    Ok(())
}

/// # Errors
/// Rejects malformed or excessive test dimensions.
pub fn test_size() -> Result<(i32, i32), Box<dyn core::error::Error>> {
    let value = std::env::var("TEHUTI_TEST_SIZE").unwrap_or_else(|_| "1280x720".into());
    let (width, height) = value.split_once('x').ok_or("size must be WIDTHxHEIGHT")?;
    let width: i32 = width.parse()?;
    let height: i32 = height.parse()?;
    if !(64..=4096).contains(&width) || !(64..=4096).contains(&height) {
        return Err("test dimensions must be between 64 and 4096".into());
    }
    Ok((width, height))
}
