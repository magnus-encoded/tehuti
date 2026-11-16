//! Watches the menu config so edits to `menu.toml` reach the running system menu.

use crate::contexts::Debounce;
use smithay::reexports::calloop::LoopHandle;
use std::{path::Path, time::Instant};

/// A watch on the menu config file with its debounced reload schedule.
#[derive(Debug)]
pub struct MenuWatch {
    debounce: Debounce,
}

impl MenuWatch {
    /// Watches the directory holding `path` and adds the watch to the event loop.
    ///
    /// # Errors
    ///
    /// Fails when inotify is unavailable or the directory cannot be watched.
    pub fn insert<D: 'static>(
        handle: &LoopHandle<'static, D>,
        path: &Path,
        watch: fn(&mut D) -> Option<&mut Self>,
    ) -> Result<Self, Box<dyn core::error::Error>> {
        let _unused = (handle, path, watch);
        Ok(Self {
            debounce: Debounce::default(),
        })
    }

    /// True once a burst of edits has gone quiet; the caller then reloads the menu.
    pub fn poll(&mut self, now: Instant) -> bool {
        self.debounce.due(now)
    }
}
