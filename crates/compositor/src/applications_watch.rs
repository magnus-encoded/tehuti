//! Watches the XDG applications directories so the destination table follows `.desktop` edits.

use crate::contexts::Debounce;
use core::mem::MaybeUninit;
use rustix::fs::inotify;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic};
use std::{
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

const WATCH_FLAGS: inotify::WatchFlags = inotify::WatchFlags::CLOSE_WRITE
    .union(inotify::WatchFlags::CREATE)
    .union(inotify::WatchFlags::DELETE)
    .union(inotify::WatchFlags::MOVED_TO)
    .union(inotify::WatchFlags::MOVED_FROM);

/// The deepest directory level watched, matching desktop entry discovery.
const MAX_DEPTH: usize = 16;

/// Whether an inotify event can change the desktop entries.
///
/// That is a `.desktop` file, a directory appearing or vanishing, or a queue overflow.
#[must_use]
pub fn relevant(name: Option<&[u8]>, is_directory: bool, overflow: bool) -> bool {
    overflow || name.is_some_and(|name| is_directory || name.ends_with(b".desktop"))
}

/// A watch on every `applications` directory under the data directories.
#[derive(Debug)]
pub struct ApplicationsWatch {
    directories: Vec<PathBuf>,
    watcher: Rc<OwnedFd>,
    debounce: Debounce,
}

impl ApplicationsWatch {
    /// Watches `<data>/applications` and its subdirectories for each data directory
    /// that has one, and adds the watch to the event loop.
    ///
    /// # Errors
    ///
    /// Fails when inotify is unavailable or the source cannot join the loop.
    pub fn insert<D: 'static>(
        handle: &LoopHandle<'static, D>,
        data: &[PathBuf],
        watch: fn(&mut D) -> Option<&mut Self>,
    ) -> Result<Self, Box<dyn core::error::Error>> {
        let watcher = Rc::new(inotify::init(
            inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC,
        )?);
        let applications_watch = Self {
            directories: data
                .iter()
                .map(|directory| directory.join("applications"))
                .collect(),
            watcher: Rc::clone(&watcher),
            debounce: Debounce::default(),
        };
        applications_watch.watch_directories();
        let _token = handle.insert_source(
            Generic::new(watcher, Interest::READ, Mode::Level),
            move |_, watcher, loop_data| {
                let Some(found) = watch(loop_data) else {
                    return Ok(PostAction::Remove);
                };
                Ok(found.read_events(watcher))
            },
        )?;
        Ok(applications_watch)
    }

    fn read_events(&mut self, watcher: &OwnedFd) -> PostAction {
        let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
        let mut reader = inotify::Reader::new(watcher, &mut buffer);
        loop {
            match reader.next() {
                Ok(event) => {
                    let is_directory = event.events().contains(inotify::ReadFlags::ISDIR);
                    let overflow = event.events().contains(inotify::ReadFlags::QUEUE_OVERFLOW);
                    let name = event.file_name().map(core::ffi::CStr::to_bytes);
                    if relevant(name, is_directory, overflow) {
                        self.debounce.changed(Instant::now());
                    }
                }
                Err(rustix::io::Errno::WOULDBLOCK | rustix::io::Errno::INTR) => return PostAction::Continue,
                Err(error) => {
                    tracing::warn!(%error, "applications watch failed; desktop entries reload only on restart");
                    return PostAction::Remove;
                }
            }
        }
    }

    fn watch_directories(&self) {
        for directory in &self.directories {
            self.watch_tree(directory, 0);
        }
    }

    fn watch_tree(&self, directory: &Path, depth: usize) {
        if depth >= MAX_DEPTH || inotify::add_watch(&*self.watcher, directory, WATCH_FLAGS).is_err() {
            return;
        }
        for child in std::fs::read_dir(directory).into_iter().flatten().flatten() {
            if child.file_type().is_ok_and(|kind| kind.is_dir()) {
                self.watch_tree(&child.path(), depth.saturating_add(1));
            }
        }
    }

    /// True once a burst of edits has gone quiet; the caller then rebuilds its destinations.
    pub fn poll(&mut self, now: Instant) -> bool {
        let due = self.debounce.due(now);
        if due {
            self.watch_directories();
        }
        due
    }
}
