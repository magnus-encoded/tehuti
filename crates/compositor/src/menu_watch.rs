//! Watches the menu config so edits to `menu.toml` reach the running system menu.

use crate::contexts::Debounce;
use core::mem::MaybeUninit;
use rustix::fs::inotify;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic};
use std::{
    ffi::OsString,
    os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::Path,
    rc::Rc,
    time::Instant,
};

const WATCH_FLAGS: inotify::WatchFlags = inotify::WatchFlags::CLOSE_WRITE
    .union(inotify::WatchFlags::CREATE)
    .union(inotify::WatchFlags::DELETE)
    .union(inotify::WatchFlags::MOVED_TO)
    .union(inotify::WatchFlags::MOVED_FROM);

/// Whether an inotify event can change the menu config: it names the file, or events were lost.
#[must_use]
pub fn relevant(name: Option<&[u8]>, file_name: &[u8], overflow: bool) -> bool {
    overflow || name == Some(file_name)
}

/// A watch on the menu config file with its debounced reload schedule.
///
/// The directory is watched, not the file, so editors that save by renaming are seen
/// and a file created later is picked up.
#[derive(Debug)]
pub struct MenuWatch {
    file_name: OsString,
    debounce: Debounce,
}

impl MenuWatch {
    /// Watches the directory holding `path` and adds the watch to the event loop.
    /// `watch` finds this value in the loop data once the caller stores it there.
    ///
    /// # Errors
    ///
    /// Fails when `path` names no file, inotify is unavailable, the directory cannot be
    /// watched, or the source cannot join the loop.
    pub fn insert<D: 'static>(
        handle: &LoopHandle<'static, D>,
        path: &Path,
        watch: fn(&mut D) -> Option<&mut Self>,
    ) -> Result<Self, Box<dyn core::error::Error>> {
        let file_name = path.file_name().ok_or("menu config path has no file name")?;
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let watcher = Rc::new(inotify::init(
            inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC,
        )?);
        let _watch_descriptor = inotify::add_watch(&*watcher, directory, WATCH_FLAGS)?;
        let _token = handle.insert_source(
            Generic::new(watcher, Interest::READ, Mode::Level),
            move |_, watcher, data| {
                let Some(found) = watch(data) else {
                    return Ok(PostAction::Remove);
                };
                Ok(found.read_events(watcher))
            },
        )?;
        Ok(Self {
            file_name: file_name.to_owned(),
            debounce: Debounce::default(),
        })
    }

    fn read_events(&mut self, watcher: &OwnedFd) -> PostAction {
        let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
        let mut reader = inotify::Reader::new(watcher, &mut buffer);
        loop {
            match reader.next() {
                Ok(event) => {
                    let overflow = event.events().contains(inotify::ReadFlags::QUEUE_OVERFLOW);
                    let name = event.file_name().map(core::ffi::CStr::to_bytes);
                    if relevant(name, self.file_name.as_bytes(), overflow) {
                        self.debounce.changed(Instant::now());
                    }
                }
                Err(rustix::io::Errno::WOULDBLOCK | rustix::io::Errno::INTR) => return PostAction::Continue,
                Err(error) => {
                    tracing::warn!(%error, "menu watch failed; the menu reloads only on restart");
                    return PostAction::Remove;
                }
            }
        }
    }

    /// True once a burst of edits has gone quiet; the caller then reloads the menu.
    pub fn poll(&mut self, now: Instant) -> bool {
        self.debounce.due(now)
    }
}
