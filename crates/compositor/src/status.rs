//! The read-only status lines under `tehuti --version`: the running session, who holds
//! the display, and the sockets in the runtime directory.

use core::fmt;
use std::path::{Path, PathBuf};

use crate::{
    drm_holders::{Holder, find},
    instance::running_pid,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// `None` when no session holds the instance lock, or the runtime directory is unknown.
    pub session_pid: Option<u32>,
    /// Whether the runtime directory was known, so "not running" can be told from "unknown".
    pub known_runtime: bool,
    /// Each DRM card with the processes that hold it open.
    pub devices: Vec<(PathBuf, Vec<Holder>)>,
    /// Socket and lock file names in the runtime directory that belong to Tehuti.
    pub sockets: Vec<String>,
}

/// Reads the status from the runtime directory, `proc_root` and the DRM device directory.
#[must_use]
pub fn gather(runtime_directory: Option<&Path>, proc_root: &Path, dri_directory: &Path) -> Status {
    let mut devices: Vec<(PathBuf, Vec<Holder>)> = std::fs::read_dir(dri_directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("card"))
        .map(|entry| entry.path())
        .map(|card| {
            let holders = find(proc_root, &card);
            (card, holders)
        })
        .filter(|(_, holders)| !holders.is_empty())
        .collect();
    devices.sort();
    let mut sockets: Vec<String> = runtime_directory
        .and_then(|directory| std::fs::read_dir(directory).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("tehuti") || name.starts_with("wayland-"))
        .collect();
    sockets.sort();
    Status {
        known_runtime: runtime_directory.is_some(),
        session_pid: runtime_directory.and_then(running_pid),
        devices,
        sockets,
    }
}

impl fmt::Display for Status {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.session_pid, self.known_runtime) {
            (Some(pid), _) => writeln!(formatter, "session: pid {pid}")?,
            (None, true) => writeln!(formatter, "session: not running")?,
            (None, false) => writeln!(formatter, "session: unknown (XDG_RUNTIME_DIR is not set)")?,
        }
        if self.devices.is_empty() {
            writeln!(formatter, "drm holders: none")?;
        } else {
            writeln!(formatter, "drm holders:")?;
            for (device, holders) in &self.devices {
                let names: Vec<String> = holders
                    .iter()
                    .map(|holder| format!("{} (pid {})", holder.command, holder.pid))
                    .collect();
                writeln!(formatter, "  {}: {}", device.display(), names.join(", "))?;
            }
        }
        if self.sockets.is_empty() {
            write!(formatter, "sockets: none")
        } else {
            write!(formatter, "sockets: {}", self.sockets.join(", "))
        }
    }
}
