//! Temporary output modes and bounded session control.

use core::time::Duration;
use std::{
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Instant,
};

/// A validated temporary mode request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Set {
        width: u16,
        height: u16,
        refresh: Option<u32>,
    },
    Restore,
}

impl Request {
    /// Parses the command argument. Refresh rates are whole Hz.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value == "--restore" {
            return Some(Self::Restore);
        }
        let (size, refresh) = match value.split_once('@') {
            Some((size, refresh)) => (size, Some(refresh.parse::<u32>().ok()?)),
            None => (value, None),
        };
        let (width, height) = size.split_once('x')?;
        let (width, height) = (width.parse::<u16>().ok()?, height.parse::<u16>().ok()?);
        (width > 0 && height > 0 && refresh != Some(0)).then_some(Self::Set {
            width,
            height,
            refresh,
        })
    }

    /// Chooses the closest dimensions, then refresh rate; ties keep driver order.
    #[must_use]
    pub fn closest(self, modes: &[(u16, u16, u32)]) -> Option<usize> {
        let Self::Set {
            width,
            height,
            refresh,
        } = self
        else {
            return None;
        };
        modes
            .iter()
            .enumerate()
            .min_by_key(|(_, (candidate_width, candidate_height, candidate_refresh))| {
                (
                    u32::from(candidate_width.abs_diff(width))
                        .saturating_add(u32::from(candidate_height.abs_diff(height))),
                    refresh.map_or(0, |refresh| candidate_refresh.abs_diff(refresh)),
                )
            })
            .map(|(index, _)| index)
    }
}

/// The first mode displaced by a temporary streaming override.
#[derive(Debug)]
pub struct Temporary<Mode> {
    saved: Option<Mode>,
}

impl<Mode> Default for Temporary<Mode> {
    fn default() -> Self {
        Self { saved: None }
    }
}

impl<Mode: Copy + PartialEq> Temporary<Mode> {
    /// Applies a temporary mode, or restores when `requested` is `None`.
    ///
    /// # Errors
    /// A rejected switch leaves the restore point unchanged.
    pub fn apply<Error>(
        &mut self,
        current: Mode,
        requested: Option<Mode>,
        switch: impl FnOnce(Mode) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let Some(target) = requested.or(self.saved) else {
            return Ok(());
        };
        if target != current {
            switch(target)?;
        }
        if requested.is_some() {
            let _saved = self.saved.get_or_insert(current);
        } else {
            self.saved = None;
        }
        Ok(())
    }
}

/// The socket belongs to the seat session, not a nested test instance.
///
/// # Errors
/// Returns an error when the runtime directory is missing.
pub fn socket_path() -> io::Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(|directory| {
            PathBuf::from(directory)
                .join("tehuti-control")
                .join("output-mode")
        })
        .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))
}

/// Sends one request and waits for the session's result.
///
/// # Errors
/// Returns socket, timeout, validation or modeset errors.
pub fn send(path: &Path, argument: &str) -> io::Result<()> {
    if Request::parse(argument).is_none() {
        return Err(io::Error::other("invalid output mode"));
    }
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "{argument}")?;
    let mut response = String::new();
    let _read = stream.take(1024).read_to_string(&mut response)?;
    if response == "ok\n" {
        Ok(())
    } else {
        Err(io::Error::other(response.trim().to_owned()))
    }
}

#[derive(Debug)]
struct Client {
    stream: UnixStream,
    bytes: Vec<u8>,
    accepted: Instant,
}

/// A private listener polled only when scanout can accept a modeset.
#[derive(Debug)]
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    clients: Vec<Client>,
}

impl Server {
    /// Binds without replacing another running session's socket.
    ///
    /// # Errors
    /// Returns filesystem or socket errors.
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("missing socket directory"))?;
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
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if !metadata.file_type().is_socket() {
                return Err(io::Error::other("control path is not a socket"));
            }
            match UnixStream::connect(&path) {
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(&path)?
                }
                _ => return Err(io::Error::other("another session owns the control socket")),
            }
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            clients: Vec::new(),
        })
    }

    /// Handles at most eight bounded requests without blocking the event loop.
    pub fn poll(&mut self, mut apply: impl FnMut(Request) -> Result<(), String>) {
        for _ in 0..8 {
            if self.clients.len() >= 8 {
                break;
            }
            let Ok((stream, _)) = self.listener.accept() else {
                break;
            };
            if stream.set_nonblocking(true).is_ok() {
                self.clients.push(Client {
                    stream,
                    bytes: Vec::new(),
                    accepted: Instant::now(),
                });
            }
        }
        self.clients.retain_mut(|client| {
            let mut buffer = [0; 128];
            match client.stream.read(&mut buffer) {
                Ok(0) => return false,
                Ok(count) => client.bytes.extend(buffer.iter().take(count)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return false,
            }
            if client.bytes.len() > 128 || client.accepted.elapsed() > Duration::from_secs(5) {
                return false;
            }
            if client.bytes.contains(&b'\n') {
                let result = std::str::from_utf8(&client.bytes)
                    .ok()
                    .and_then(|value| value.strip_suffix('\n'))
                    .and_then(Request::parse)
                    .ok_or_else(|| "invalid output mode".to_owned())
                    .and_then(&mut apply);
                let response =
                    result.map_or_else(|error| format!("error: {error}\n"), |()| "ok\n".to_owned());
                let _sent = client.stream.write_all(response.as_bytes());
                return false;
            }
            true
        });
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _removed = std::fs::remove_file(&self.path);
        if let Some(directory) = self.path.parent() {
            let _removed = std::fs::remove_dir(directory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_supported_dimensions_precede_refresh_and_refresh_is_optional() {
        let modes = [(1920, 1080, 60), (1280, 720, 60), (1280, 720, 120)];
        assert_eq!(
            Request::parse("1280x720@100").and_then(|request| request.closest(&modes)),
            Some(2)
        );
        assert_eq!(
            Request::parse("1300x740@60").and_then(|request| request.closest(&modes)),
            Some(1)
        );
        assert_eq!(
            Request::parse("1280x720").and_then(|request| request.closest(&modes)),
            Some(1)
        );
        assert_eq!(
            Request::parse("1280x720@60").and_then(|request| request.closest(&[])),
            None
        );
        for value in [
            "0x720",
            "1280x0",
            "1280x720@0",
            "65536x720",
            "1280x720@60junk",
            "1280x720@-1",
        ] {
            assert_eq!(Request::parse(value), None);
        }
    }

    #[test]
    fn private_socket_delivers_requests_and_reports_modeset_errors() {
        let runtime = tempfile::tempdir().unwrap();
        let path = runtime.path().join("control").join("output-mode");
        let mut server = Server::bind(path.clone()).unwrap();
        let client = std::thread::spawn(move || send(&path, "1280x720@60"));
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut received = Vec::new();
        while !client.is_finished() && Instant::now() < deadline {
            server.poll(|request| {
                received.push(request);
                Err("modeset rejected".to_owned())
            });
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            received,
            [Request::Set {
                width: 1280,
                height: 720,
                refresh: Some(60)
            }]
        );
        assert!(
            client
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("modeset rejected")
        );
    }
    #[test]
    fn repeated_requests_restore_the_original_mode_and_restore_is_idempotent() {
        let mut session = Temporary::default();
        let mut current = 1920;
        for requested in [Some(1280), Some(2560), None, None] {
            session
                .apply(current, requested, |target| {
                    current = target;
                    Ok::<_, ()>(())
                })
                .unwrap();
        }
        assert_eq!(current, 1920);
        session
            .apply(current, Some(3840), |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        session
            .apply(current, None, |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(current, 1920);
    }

    #[test]
    fn failed_switches_preserve_the_restore_point() {
        let mut session = Temporary::default();
        assert_eq!(
            session.apply(1920, Some(1280), |_| Err("rejected")),
            Err("rejected")
        );
        let mut current = 2560;
        session
            .apply(current, None, |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(current, 2560);
        session
            .apply(current, Some(1280), |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(session.apply(current, None, |_| Err("rejected")), Err("rejected"));
        session
            .apply(current, None, |target| {
                current = target;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(current, 2560);
    }
}
