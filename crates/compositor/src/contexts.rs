//! Loads independent context files, containing failures to each file.

use core::time::Duration;
use std::{
    io::{self, Read},
    os::{fd::OwnedFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    time::Instant,
};

const MAX_CONTEXT_BYTES: u32 = 64 * 1024;

use tehuti_protocol::{Context, parse_context};

/// Loads `context.toml` in the directory and its immediate child directories.
///
/// Paths are sorted; the first context wins when application ids overlap.
/// A broken file is skipped with a warning so startup always gets the valid contexts.
#[must_use]
pub fn load(directory: &Path) -> Vec<Context> {
    let mut contexts = Vec::new();
    for path in context_paths(directory) {
        match load_one(&path) {
            Ok(Some(context)) => contexts.push(context),
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "context skipped"),
        }
    }
    contexts
}

/// Loads every context or none: the first broken file fails the whole reload.
///
/// # Errors
///
/// Names the first file that cannot be read or parsed.
pub fn try_load(directory: &Path) -> Result<Vec<Context>, LoadError> {
    let mut contexts = Vec::new();
    for path in context_paths(directory) {
        if let Some(context) = load_one(&path)? {
            contexts.push(context);
        }
    }
    Ok(contexts)
}

/// A context file that could not be read or parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

impl LoadError {
    /// Two short notice lines: the file, then the start of the error.
    #[must_use]
    pub fn notice(&self) -> String {
        let file = self.path.parent().and_then(Path::file_name).map_or_else(
            || self.path.display().to_string(),
            |name| format!("{}/context.toml", name.display()),
        );
        let first_line = self.message.lines().next().unwrap_or_default();
        format!(
            "CONTEXT ERROR {file}\n{}",
            first_line.chars().take(40).collect::<String>()
        )
    }
}

fn context_paths(directory: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(?directory, %error, "context directory unavailable");
            return Vec::new();
        }
    };
    let mut paths = vec![directory.join("context.toml")];
    for entry in entries {
        match entry {
            Ok(entry) if entry.path().is_dir() => paths.push(entry.path().join("context.toml")),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "context directory entry skipped"),
        }
    }
    paths.sort();
    paths
}

fn load_one(path: &Path) -> Result<Option<Context>, LoadError> {
    let failed = |message: String| LoadError {
        path: path.to_path_buf(),
        message,
    };
    match read_context(path) {
        Ok(source) => parse_context(&source).map(Some).map_err(failed),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failed(error.to_string())),
    }
}

/// Trailing debounce: a burst of changes yields one reload once it goes quiet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Debounce {
    deadline: Option<Instant>,
}

/// How long the context directory must stay quiet before a reload.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

impl Debounce {
    /// Records a change, pushing the reload back until the directory is quiet again.
    pub fn changed(&mut self, now: Instant) {
        self.deadline = now.checked_add(DEBOUNCE);
    }

    /// True once per burst, when the quiet period has passed. Clears the schedule.
    pub fn due(&mut self, now: Instant) -> bool {
        let due = self.deadline.is_some_and(|deadline| now >= deadline);
        if due {
            self.deadline = None;
        }
        due
    }
}

/// Watches the context directory and its child directories for context edits.
///
/// # Errors
///
/// Fails when inotify is unavailable or the directory cannot be watched.
pub fn watch(directory: &Path) -> io::Result<OwnedFd> {
    use rustix::fs::inotify;
    let watcher = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC)?;
    rewatch(&watcher, directory)?;
    Ok(watcher)
}

/// Adds watches for the directory and any child directory, including new ones.
///
/// # Errors
///
/// Fails when the top directory cannot be watched.
pub fn rewatch(watcher: &OwnedFd, directory: &Path) -> io::Result<()> {
    use rustix::fs::inotify;
    let flags = inotify::WatchFlags::CLOSE_WRITE
        | inotify::WatchFlags::CREATE
        | inotify::WatchFlags::DELETE
        | inotify::WatchFlags::MOVED_TO
        | inotify::WatchFlags::MOVED_FROM;
    let _watch = inotify::add_watch(watcher, directory, flags)?;
    for path in context_paths(directory) {
        if let Some(parent) = path.parent().filter(|parent| *parent != directory)
            && let Err(error) = inotify::add_watch(watcher, parent, flags)
        {
            tracing::warn!(?parent, %error, "context directory not watched");
        }
    }
    Ok(())
}

fn read_context(path: &Path) -> io::Result<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "context must be a regular file",
        ));
    }
    let mut source = String::new();
    let bytes_read = file
        .take(u64::from(MAX_CONTEXT_BYTES) + 1)
        .read_to_string(&mut source)?;
    if bytes_read > usize::try_from(MAX_CONTEXT_BYTES).unwrap_or(usize::MAX) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "context exceeds 64 KiB",
        ));
    }
    Ok(source)
}
