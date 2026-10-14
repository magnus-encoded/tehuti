//! Loads independent context files, containing failures to each file.

use core::mem::MaybeUninit;
use core::time::Duration;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic};
use std::{
    io::{self, Read},
    os::{fd::OwnedFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    rc::Rc,
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
    let paths = context_paths(directory, false).unwrap_or_else(|error| {
        tracing::warn!(%error, "context directory unavailable");
        Vec::new()
    });
    let mut contexts = Vec::new();
    for path in paths {
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
    for path in context_paths(directory, true)? {
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
    /// Two short notice lines: the file, then the error location and message.
    #[must_use]
    pub fn notice(&self) -> String {
        let file = self.path.parent().and_then(Path::file_name).map_or_else(
            || self.path.display().to_string(),
            |name| format!("{}/context.toml", name.display()),
        );
        format!("CONTEXT ERROR {file}\n{}", short_error(&self.message))
    }
}

/// Condenses a TOML error to `L<line> C<column> <message>`, at most 40 characters.
///
/// TOML errors put the location on the first line, a source excerpt with `|` gutters
/// next, and the message after it. Other errors keep their first line.
#[must_use]
pub fn short_error(message: &str) -> String {
    let mut lines = message.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default();
    let location = first.split_once(" at line ").and_then(|(_, rest)| {
        let (line, column) = rest.split_once(", column ")?;
        Some(format!("L{line} C{column}"))
    });
    let detail = lines
        .find(|line| !line.contains('|'))
        .filter(|_| location.is_some())
        .unwrap_or(first);
    let text = location.map_or_else(|| detail.to_owned(), |location| format!("{location} {detail}"));
    text.chars().take(40).collect()
}

/// The candidate `context.toml` paths, sorted. Any directory read error fails the listing.
fn context_paths(directory: &Path, strict: bool) -> Result<Vec<PathBuf>, LoadError> {
    let failed = |path: &Path, error: &io::Error| LoadError {
        path: path.to_path_buf(),
        message: error.to_string(),
    };
    let entries = std::fs::read_dir(directory).map_err(|error| failed(directory, &error))?;
    let mut paths = vec![directory.join("context.toml")];
    for entry in entries {
        let listed = entry
            .map_err(|error| failed(directory, &error))
            .and_then(|entry| {
                let path = entry.path();
                match std::fs::metadata(&path) {
                    Ok(metadata) if metadata.is_dir() => Ok(Some(path.join("context.toml"))),
                    Ok(_) => Ok(None),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(failed(&path, &error)),
                }
            });
        match listed {
            Ok(path) => paths.extend(path),
            Err(error) if !strict => tracing::warn!(%error, "context directory entry skipped"),
            Err(error) => return Err(error),
        }
    }
    paths.sort();
    Ok(paths)
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

/// A watch on the context directory with its debounced reload schedule.
#[derive(Debug)]
pub struct ContextWatch {
    directory: PathBuf,
    watcher: Rc<OwnedFd>,
    top: i32,
    debounce: Debounce,
}

const WATCH_FLAGS: rustix::fs::inotify::WatchFlags = rustix::fs::inotify::WatchFlags::CLOSE_WRITE
    .union(rustix::fs::inotify::WatchFlags::CREATE)
    .union(rustix::fs::inotify::WatchFlags::DELETE)
    .union(rustix::fs::inotify::WatchFlags::MOVED_TO)
    .union(rustix::fs::inotify::WatchFlags::MOVED_FROM);

/// Whether an inotify event can change the loaded contexts.
///
/// That is a `context.toml` anywhere watched, a directory appearing or vanishing in
/// the top directory, or a queue overflow, after which any change may have been lost.
#[must_use]
pub fn relevant(name: Option<&[u8]>, is_directory: bool, in_top: bool, overflow: bool) -> bool {
    overflow || name == Some(b"context.toml".as_slice()) || (is_directory && in_top && name.is_some())
}

impl ContextWatch {
    /// Watches the directory and its child directories, and adds the watch to the event loop.
    /// `watch` finds this value in the loop data once the caller stores it there.
    ///
    /// # Errors
    ///
    /// Fails when inotify is unavailable, the directory cannot be watched, or the
    /// source cannot join the loop.
    pub fn insert<D: 'static>(
        handle: &LoopHandle<'static, D>,
        directory: &Path,
        watch: fn(&mut D) -> Option<&mut Self>,
    ) -> Result<Self, Box<dyn core::error::Error>> {
        use rustix::fs::inotify;
        let watcher = Rc::new(inotify::init(
            inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC,
        )?);
        let top = inotify::add_watch(&*watcher, directory, WATCH_FLAGS)?;
        let context_watch = Self {
            directory: directory.to_path_buf(),
            watcher: Rc::clone(&watcher),
            top,
            debounce: Debounce::default(),
        };
        context_watch.watch_children();
        let _token = handle.insert_source(
            Generic::new(watcher, Interest::READ, Mode::Level),
            move |_, watcher, data| {
                let Some(found) = watch(data) else {
                    return Ok(PostAction::Remove);
                };
                Ok(found.read_events(watcher))
            },
        )?;
        Ok(context_watch)
    }

    fn read_events(&mut self, watcher: &OwnedFd) -> PostAction {
        let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
        let mut reader = rustix::fs::inotify::Reader::new(watcher, &mut buffer);
        loop {
            match reader.next() {
                Ok(event) => {
                    let is_directory = event.events().contains(rustix::fs::inotify::ReadFlags::ISDIR);
                    let name = event.file_name().map(core::ffi::CStr::to_bytes);
                    let overflow = event
                        .events()
                        .contains(rustix::fs::inotify::ReadFlags::QUEUE_OVERFLOW);
                    if relevant(name, is_directory, event.wd() == self.top, overflow) {
                        self.debounce.changed(Instant::now());
                    }
                }
                Err(rustix::io::Errno::WOULDBLOCK | rustix::io::Errno::INTR) => return PostAction::Continue,
                Err(error) => {
                    tracing::warn!(%error, "context watch failed; contexts reload only on restart");
                    return PostAction::Remove;
                }
            }
        }
    }

    fn watch_children(&self) {
        let paths = match context_paths(&self.directory, false) {
            Ok(paths) => paths,
            Err(error) => {
                tracing::warn!(%error, "context directories not watched");
                return;
            }
        };
        for path in paths {
            if let Some(parent) = path.parent().filter(|parent| *parent != self.directory)
                && let Err(error) = rustix::fs::inotify::add_watch(&*self.watcher, parent, WATCH_FLAGS)
            {
                tracing::warn!(?parent, %error, "context directory not watched");
            }
        }
    }

    /// The reload result once a burst of edits has gone quiet, else `None`.
    pub fn poll(&mut self, now: Instant) -> Option<Result<Vec<Context>, LoadError>> {
        self.debounce.due(now).then(|| {
            self.watch_children();
            try_load(&self.directory)
        })
    }
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
