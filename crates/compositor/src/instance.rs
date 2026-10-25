//! One Tehuti per user: an exclusive `flock` on a file in the user's runtime directory.
//!
//! The kernel drops the lock when the holder exits, so a crashed instance never blocks
//! the next start.

use std::{
    fs::File,
    io::{Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
};

use crate::backend::Backend;
use rustix::fs::{FlockOperation, flock};

pub const LOCK_FILE_NAME: &str = "tehuti.lock";

/// Held for the life of the process; dropping it lets another instance start.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error("XDG_RUNTIME_DIR is not set; Tehuti needs it for its lock and Wayland socket")]
    NoRuntimeDirectory,
    #[error(
        "another Tehuti is already running for this user{}; its lock is {}",
        .pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default(),
        .path.display()
    )]
    AlreadyRunning { path: PathBuf, pid: Option<u32> },
    #[error("cannot use the instance lock {}: {source}", .path.display())]
    Lock { path: PathBuf, source: std::io::Error },
}

/// Takes the lock in `$XDG_RUNTIME_DIR` for a real session (`Backend::Tty`).
///
/// A nested start under another compositor or X takes no lock, so it can run beside the
/// session it is tested from.
///
/// # Errors
/// Returns an error when the runtime directory is unset or another instance holds the lock.
pub fn acquire_from_env(backend: Backend) -> Result<Option<InstanceLock>, InstanceError> {
    if backend == Backend::Nested {
        return Ok(None);
    }
    let runtime_directory = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .ok_or(InstanceError::NoRuntimeDirectory)?;
    acquire_for(backend, Path::new(&runtime_directory))
}

/// Takes the lock in `runtime_directory` unless the backend is nested.
///
/// # Errors
/// Returns an error when another instance holds the lock.
pub fn acquire_for(
    backend: Backend,
    runtime_directory: &Path,
) -> Result<Option<InstanceLock>, InstanceError> {
    match backend {
        Backend::Nested => Ok(None),
        Backend::Tty => acquire(runtime_directory).map(Some),
    }
}

/// Takes the lock in `runtime_directory` and records this process id in it.
///
/// # Errors
/// Returns [`InstanceError::AlreadyRunning`] while another holder has the lock,
/// and [`InstanceError::Lock`] when the file cannot be opened or written.
pub fn acquire(runtime_directory: &Path) -> Result<InstanceLock, InstanceError> {
    let path = runtime_directory.join(LOCK_FILE_NAME);
    let failed = |source: std::io::Error| InstanceError::Lock {
        path: path.clone(),
        source,
    };
    let mut file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(failed)?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(rustix::io::Errno::WOULDBLOCK) => {
            let mut recorded = String::new();
            let pid = file
                .read_to_string(&mut recorded)
                .ok()
                .and_then(|_| recorded.trim().parse().ok());
            return Err(InstanceError::AlreadyRunning { path, pid });
        }
        Err(error) => return Err(failed(error.into())),
    }
    file.set_len(0).map_err(failed)?;
    file.rewind().map_err(failed)?;
    writeln!(file, "{}", std::process::id()).map_err(failed)?;
    Ok(InstanceLock { _file: file })
}
