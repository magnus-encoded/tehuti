//! Session idle inhibition for Stay Awake mode, off the compositor thread (ADR 0007).

use std::{io, sync::mpsc};

/// Worker managing a systemd-logind idle/sleep inhibitor lock via D-Bus.
#[derive(Debug)]
pub struct Backend {
    requests: mpsc::Sender<bool>,
    replies: mpsc::Receiver<Result<(), String>>,
}

impl Backend {
    /// Starts the production D-Bus idle-inhibit worker thread.
    ///
    /// # Errors
    /// The worker thread could not be started.
    pub fn start() -> io::Result<Self> {
        let mut lock: Option<zbus::zvariant::OwnedFd> = None;
        Self::with_handler(move |active| {
            if active {
                if lock.is_none() {
                    lock = Some(acquire_lock()?);
                }
            } else {
                lock = None;
            }
            Ok(())
        })
    }

    /// Starts a worker driven by `handler`, for tests without a real system bus.
    ///
    /// # Errors
    /// The worker thread could not be started.
    pub fn with_handler(
        mut handler: impl FnMut(bool) -> Result<(), String> + Send + 'static,
    ) -> io::Result<Self> {
        let (requests, input) = mpsc::channel();
        let (output, replies) = mpsc::channel();
        let _worker = std::thread::Builder::new()
            .name("idle-inhibit".into())
            .spawn(move || {
                while let Ok(active) = input.recv() {
                    if output.send(handler(active)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { requests, replies })
    }

    /// Requests acquiring (`true`) or releasing (`false`) the idle inhibitor.
    ///
    /// # Errors
    /// The worker exited.
    pub fn set(&self, active: bool) -> Result<(), String> {
        self.requests.send(active).map_err(|error| error.to_string())
    }

    pub fn poll(&self) -> impl Iterator<Item = Result<(), String>> + '_ {
        self.replies.try_iter()
    }
}

fn acquire_lock() -> Result<zbus::zvariant::OwnedFd, String> {
    let connection = zbus::blocking::Connection::system().map_err(|error| error.to_string())?;
    let reply = connection
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "Inhibit",
            &("idle:sleep", "Tehuti", "Stay awake mode", "block"),
        )
        .map_err(|error| error.to_string())?;
    reply
        .body()
        .deserialize::<zbus::zvariant::OwnedFd>()
        .map_err(|error| error.to_string())
}
