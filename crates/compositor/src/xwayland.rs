//! Optional X11 satellite discovery and supervision.

use std::{
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
};

/// Chooses the first executable satellite in PATH order.
#[must_use]
pub fn find_binary(directories: &[PathBuf], executable: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    directories
        .iter()
        .map(|directory| directory.join("xwayland-satellite"))
        .find(|path| executable(path))
}

/// Chooses a free local X11 display from the first 1024 numbers.
#[must_use]
pub fn choose_display(occupied: impl Fn(u16) -> bool) -> Option<u16> {
    (0..1024).find(|number| !occupied(*number))
}

/// Discovers the installed satellite and a free display without starting a process.
#[must_use]
pub fn discover() -> Option<(PathBuf, String)> {
    use std::os::unix::fs::PermissionsExt as _;
    let directories: Vec<_> =
        std::env::var_os("PATH").map_or_else(Vec::new, |path| std::env::split_paths(&path).collect());
    let Some(binary) = find_binary(&directories, |path| {
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }) else {
        tracing::info!("xwayland-satellite is not installed; staying Wayland-only");
        return None;
    };
    let Some(number) = choose_display(|number| {
        [format!("/tmp/.X{number}-lock"), format!("/tmp/.X11-unix/X{number}")].iter().any(|path| {
            !matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        })
    }) else {
        tracing::warn!("no free X11 display; staying Wayland-only");
        return None;
    };
    Some((binary, format!(":{number}")))
}

#[derive(Debug)]
pub struct Satellite {
    binary: PathBuf,
    display: String,
    socket: std::ffi::OsString,
    child: Option<std::process::Child>,
    restart: crate::child_process::Restart,
    keymap: KeymapSync,
}

impl Satellite {
    #[must_use]
    pub fn new(binary: PathBuf, display: String, socket: std::ffi::OsString) -> Self {
        Self {
            binary,
            display,
            socket,
            child: None,
            restart: crate::child_process::Restart::default(),
            keymap: KeymapSync::default(),
        }
    }

    /// Applies the compositor map and reports whether the upload has completed.
    pub fn sync_keymap(&mut self, elapsed: core::time::Duration, keymap: &str) -> bool {
        if self.child.is_none() {
            return false;
        }
        self.keymap.poll(elapsed, &self.display, keymap);
        self.keymap.pending.is_none() && self.keymap.applied == keymap
    }

    /// Polls child status and starts due attempts without waiting for child exit.
    pub fn poll(&mut self, elapsed: core::time::Duration) {
        self.restart.advance(elapsed, self.child.is_some());
        if let Some(running) = self.child.as_mut() {
            match running.try_wait() {
                Ok(None) => return,
                Ok(Some(status)) => tracing::warn!(%status, "xwayland-satellite exited"),
                Err(error) => tracing::warn!(%error, "xwayland-satellite status failed"),
            }
            if let Some(mut child) = self.child.take() {
                crate::shutdown::terminate(&mut child, true);
            }
            self.keymap = KeymapSync::default();
            self.restart.failed();
            return;
        }
        if !self.restart.ready() {
            return;
        }
        match crate::child_process::command(&self.binary)
            .arg(&self.display)
            .env("WAYLAND_DISPLAY", &self.socket)
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                tracing::info!(display = %self.display, pid = child.id(), "xwayland-satellite started");
                self.child = Some(child);
            }
            Err(error) => {
                tracing::warn!(%error, "xwayland-satellite start failed");
                self.restart.failed();
            }
        }
    }
}

impl Drop for Satellite {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            crate::shutdown::terminate(&mut child, true);
        }
    }
}

#[derive(Debug, Default)]
struct KeymapSync {
    applied: String,
    pending: Option<(std::process::Child, String)>,
    elapsed: core::time::Duration,
    retry: core::time::Duration,
}

impl KeymapSync {
    fn poll(&mut self, elapsed: core::time::Duration, display: &str, keymap: &str) {
        self.retry = self.retry.saturating_sub(elapsed);
        if let Some((child, submitted)) = self.pending.as_mut() {
            self.elapsed = self.elapsed.saturating_add(elapsed);
            match child.try_wait() {
                Ok(None) if self.elapsed < core::time::Duration::from_secs(5) => return,
                Ok(Some(status)) if status.success() => self.applied.clone_from(submitted),
                result => {
                    tracing::warn!(?result, "X11 keymap update failed; retrying");
                    crate::shutdown::terminate(child, false);
                    self.retry = core::time::Duration::from_secs(1);
                }
            }
            self.pending = None;
        }
        if self.applied == keymap || !self.retry.is_zero() {
            return;
        }
        match upload_keymap(display, keymap) {
            Ok(child) => {
                self.pending = Some((child, keymap.to_owned()));
                self.elapsed = core::time::Duration::ZERO;
            }
            Err(error) => {
                tracing::warn!(%error, "X11 keymap upload unavailable; retrying");
                self.retry = core::time::Duration::from_secs(5);
            }
        }
    }
}

fn upload_keymap(display: &str, keymap: &str) -> std::io::Result<std::process::Child> {
    use std::os::fd::AsFd as _;
    let file = smithay::utils::SealedFile::with_data(c"tehuti-x11-keymap", keymap.as_bytes())?;
    crate::child_process::command("xkbcomp")
        .args(["-w", "0", "-", display])
        .stdin(std::process::Stdio::from(file.as_fd().try_clone_to_owned()?))
        .stdout(std::process::Stdio::null())
        .spawn()
}

impl Drop for KeymapSync {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.pending.take() {
            crate::shutdown::terminate(&mut child, false);
        }
    }
}
