//! Session Restore: which applications were open, and where, saved on a clean exit and
//! relaunched on the next start.
//!
//! The file lives under the XDG state directory and holds only desktop entry ids with their
//! place, never commands: reading it can only start an installed application.

use core::time::Duration;
use std::{collections::VecDeque, io, io::Write as _, path::PathBuf, time::Instant};

use serde::{Deserialize, Serialize};
use tehuti_core::layout::{Snapshot, WindowId};

const VERSION: u32 = 1;
/// How long a restored application may take to open its window before the next one starts.
const WINDOW_WAIT: Duration = Duration::from_secs(5);

/// One saved window: the desktop entry to launch and the place it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedWindow {
    /// The window's app id, which is also the desktop entry id used to launch it.
    pub app_id: String,
    /// Position among the non-empty workspaces of the output.
    pub workspace: usize,
    /// Column position inside that workspace.
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    version: u32,
    windows: Vec<SavedWindow>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("session file unreadable: {0}")]
    Io(#[from] io::Error),
    #[error("session file invalid: {0}")]
    Invalid(String),
}

impl Saved {
    /// The restorable windows of the first output, in layout order. Windows whose app id is
    /// not an installed desktop entry are left out, as are helpers.
    #[must_use]
    pub fn capture(
        snapshot: &Snapshot,
        app_id_of: impl Fn(WindowId) -> Option<String>,
        installed: &[String],
    ) -> Self {
        let mut windows = Vec::new();
        let workspaces = snapshot.outputs.first().map(|output| output.workspaces.as_slice());
        let mut rank = 0;
        for workspace in workspaces.unwrap_or_default() {
            let before = windows.len();
            for (column, view) in workspace.columns.iter().enumerate() {
                for tile in &view.tiles {
                    if let Some(app_id) = app_id_of(tile.window).filter(|app_id| installed.contains(app_id)) {
                        windows.push(SavedWindow { app_id, workspace: rank, column });
                    }
                }
            }
            if windows.len() > before {
                rank = rank.saturating_add(1);
            }
        }
        Self { version: VERSION, windows }
    }

    #[must_use]
    pub fn windows(&self) -> &[SavedWindow] {
        &self.windows
    }

    #[must_use]
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// # Errors
    /// The text is not a version 1 session file with plain desktop entry ids.
    pub fn parse(text: &str) -> Result<Self, LoadError> {
        let saved: Self = serde_json::from_str(text).map_err(|error| LoadError::Invalid(error.to_string()))?;
        if saved.version != VERSION {
            return Err(LoadError::Invalid(format!("unsupported version {}", saved.version)));
        }
        let plain = |id: &str| {
            !id.is_empty() && !id.starts_with('-') && !id.contains('/') && !id.contains('\0')
        };
        if saved.windows.iter().any(|window| !plain(&window.app_id)) {
            return Err(LoadError::Invalid("invalid desktop entry id".into()));
        }
        Ok(saved)
    }
}

/// The session file, replaced atomically.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `$XDG_STATE_HOME/tehuti/session.json`, otherwise under `$HOME/.local/state`.
    ///
    /// # Errors
    /// Neither the state directory nor home is available.
    pub fn from_env() -> io::Result<Self> {
        let root = std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "state directory unavailable"))?;
        Ok(Self::new(root.join("tehuti/session.json")))
    }

    /// # Errors
    /// Directory creation, write or atomic replacement failed.
    pub fn save(&self, saved: &Saved) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("state directory unavailable"))?;
        std::fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("tmp");
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(saved.to_text().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)
    }

    /// Reads the saved session and removes the file, so one save restores once and a bad
    /// file warns once.
    ///
    /// # Errors
    /// The file is unreadable or invalid; it is removed anyway.
    pub fn take(&self) -> Result<Option<Saved>, LoadError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(%error, "session file not removed");
        }
        Saved::parse(&text).map(Some)
    }
}

#[derive(Debug)]
struct Awaiting {
    app_id: String,
    open_before: usize,
    deadline: Instant,
}

/// Hands out saved windows one at a time, so each lands next to the previous one.
#[derive(Debug)]
pub struct Restore {
    pending: VecDeque<SavedWindow>,
    seen: Vec<(String, usize)>,
    awaiting: Option<Awaiting>,
}

impl Restore {
    #[must_use]
    pub fn new(saved: Saved) -> Self {
        Self {
            pending: saved.windows.into(),
            seen: Vec::new(),
            awaiting: None,
        }
    }

    #[must_use]
    pub fn is_done(&self) -> bool {
        self.pending.is_empty() && self.awaiting.is_none()
    }

    /// The next window to launch, given the app ids of the windows open now. Waits for the
    /// previous launch to open a window or time out; skips windows already open.
    pub fn next(&mut self, now: Instant, open: &[String]) -> Option<SavedWindow> {
        let count = |app_id: &str| open.iter().filter(|open| open.as_str() == app_id).count();
        if let Some(awaiting) = &self.awaiting {
            if count(&awaiting.app_id) <= awaiting.open_before && now < awaiting.deadline {
                return None;
            }
            self.awaiting = None;
        }
        while let Some(window) = self.pending.pop_front() {
            let nth = match self.seen.iter_mut().find(|(app_id, _)| *app_id == window.app_id) {
                Some((_, seen)) => {
                    *seen = seen.saturating_add(1);
                    *seen
                }
                None => {
                    self.seen.push((window.app_id.clone(), 1));
                    1
                }
            };
            let open_now = count(&window.app_id);
            if open_now >= nth {
                continue;
            }
            self.awaiting = Some(Awaiting {
                app_id: window.app_id.clone(),
                open_before: open_now,
                deadline: now.checked_add(WINDOW_WAIT).unwrap_or(now),
            });
            return Some(window);
        }
        None
    }
}
