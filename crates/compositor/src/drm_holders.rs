//! Which processes hold the DRM device open, read from `/proc`.
//!
//! A start that finds the device busy names the holders instead of failing silently.

use core::fmt;
use std::path::{Path, PathBuf};

/// A process with the device open.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Holder {
    pub pid: u32,
    pub command: String,
}

/// Every process under `proc_root` with an open descriptor pointing at `device`, by pid.
#[must_use]
pub fn find(proc_root: &Path, device: &Path) -> Vec<Holder> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut holders: Vec<Holder> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let open = std::fs::read_dir(entry.path().join("fd"))
                .ok()?
                .filter_map(Result::ok)
                .any(|descriptor| std::fs::read_link(descriptor.path()).is_ok_and(|target| target == device));
            open.then(|| Holder {
                pid,
                command: std::fs::read_to_string(entry.path().join("comm"))
                    .map_or_default(|name| name.trim().to_owned()),
            })
        })
        .collect();
    holders.sort();
    holders
}

/// The start could not take the device because another process holds it.
#[derive(Debug)]
pub struct DeviceBusy {
    pub device: PathBuf,
    pub holders: Vec<Holder>,
}

impl fmt::Display for DeviceBusy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} is held by another process", self.device.display())?;
        if self.holders.is_empty() {
            write!(formatter, "; no other process was found holding it")?;
        }
        for holder in &self.holders {
            write!(formatter, "; {} (pid {})", holder.command, holder.pid)?;
        }
        write!(
            formatter,
            "; close the program holding the display, then start Tehuti again"
        )
    }
}

impl core::error::Error for DeviceBusy {}

/// Whether `error`, or anything in its source chain, reports a busy device.
#[must_use]
pub fn is_busy(error: &(dyn core::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(link) = current {
        if link.is::<DeviceBusy>()
            || link
                .downcast_ref::<std::io::Error>()
                .is_some_and(|failure| failure.raw_os_error() == Some(16))
            || format!("{link:?}").contains("ResourceBusy")
        {
            return true;
        }
        current = link.source();
    }
    false
}
