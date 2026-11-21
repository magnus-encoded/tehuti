//! Session Restore: which applications were open, and where, saved on a clean exit and
//! relaunched on the next start.
//!
//! The file lives under the XDG state directory and holds only desktop entry ids with their
//! place, never commands: reading it can only start an installed application.

use core::time::Duration;
use std::{io, io::Read as _, io::Write as _, path::PathBuf, time::Instant};

use serde::{Deserialize, Serialize};
use tehuti_core::layout::{OpenAt, Snapshot, WindowId};

const VERSION: u32 = 1;
/// How long a launched application has to open its window before its place is forgotten.
const PLACEMENT_LIFETIME: Duration = Duration::from_secs(60);
/// The largest session file read.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// The most windows saved or restored.
pub const MAX_WINDOWS: usize = 64;

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

#[derive(Debug)]
pub enum LoadError {
    Unreadable(io::Error),
    Invalid(String),
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unreadable(error) => write!(formatter, "session file unreadable: {error}"),
            Self::Invalid(reason) => write!(formatter, "session file invalid: {reason}"),
        }
    }
}

impl core::error::Error for LoadError {}

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
        let workspaces = snapshot
            .outputs
            .first()
            .map(|output| output.workspaces.as_slice());
        let mut rank = 0;
        for workspace in workspaces.unwrap_or_default() {
            let before = windows.len();
            for (column, view) in workspace.columns.iter().enumerate() {
                for tile in &view.tiles {
                    if windows.len() < MAX_WINDOWS
                        && let Some(app_id) =
                            app_id_of(tile.window).filter(|app_id| installed.contains(app_id))
                    {
                        windows.push(SavedWindow {
                            app_id,
                            workspace: rank,
                            column,
                        });
                    }
                }
            }
            if windows.len() > before {
                rank = rank.saturating_add(1);
            }
        }
        Self {
            version: VERSION,
            windows,
        }
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
        let saved: Self =
            serde_json::from_str(text).map_err(|error| LoadError::Invalid(error.to_string()))?;
        if saved.windows.len() > MAX_WINDOWS {
            return Err(LoadError::Invalid(format!("more than {MAX_WINDOWS} windows")));
        }
        if saved.version != VERSION {
            return Err(LoadError::Invalid(format!(
                "unsupported version {}",
                saved.version
            )));
        }
        let plain = |entry: &str| {
            !entry.is_empty() && !entry.starts_with('-') && !entry.contains('/') && !entry.contains('\0')
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
        let metadata = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(LoadError::Unreadable(error)),
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            self.discard();
            return Err(LoadError::Invalid("not a regular file under the size cap".into()));
        }
        let mut text = String::new();
        let read = std::fs::File::open(&self.path).and_then(|file| {
            file.take(MAX_FILE_BYTES.saturating_add(1))
                .read_to_string(&mut text)
        });
        self.discard();
        read.map_err(LoadError::Unreadable)?;
        Saved::parse(&text).map(Some)
    }

    fn discard(&self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(%error, "session file not removed");
        }
    }
}

/// Where each launched application's windows still have to go. A window that opens claims
/// the first entry for its app id; entries nobody claims expire.
#[derive(Debug)]
pub struct Placements {
    pending: Vec<(SavedWindow, Instant)>,
}

impl Placements {
    #[must_use]
    pub fn new(saved: &Saved, now: Instant) -> Self {
        let expires = now.checked_add(PLACEMENT_LIFETIME).unwrap_or(now);
        Self {
            pending: saved
                .windows
                .iter()
                .map(|window| (window.clone(), expires))
                .collect(),
        }
    }

    /// The saved place for a window that just opened, removed from the list. `None` for a
    /// window nothing was saved for, or whose entry expired: it opens as usual.
    pub fn claim(&mut self, app_id: &str, now: Instant) -> Option<OpenAt> {
        self.pending.retain(|(_, expires)| now < *expires);
        let index = self
            .pending
            .iter()
            .position(|(window, _)| window.app_id == app_id)?;
        let (window, _) = self.pending.remove(index);
        Some(OpenAt {
            workspace: window.workspace,
            column: window.column,
        })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Whether the exit writes the session: only a clean quit, with Session Restore on.
#[must_use]
pub const fn save_due(loop_ok: bool, quit_requested: bool, enabled: bool) -> bool {
    loop_ok && quit_requested && enabled
}

/// Reads `restore_session = true | false` from the text of `session.toml`.
///
/// # Errors
/// The text is not TOML or the value is not a boolean.
pub fn parse_enabled(text: &str) -> Result<bool, String> {
    let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
    match table.get("restore_session").map(toml::Value::as_bool) {
        None => Ok(false),
        Some(Some(enabled)) => Ok(enabled),
        Some(None) => Err("restore_session must be true or false".into()),
    }
}

/// Whether Session Restore is on: `session.toml` in the config directory, off when missing or bad.
#[must_use]
pub fn enabled() -> bool {
    let Ok(root) = crate::display_mode_store::config_root() else {
        return false;
    };
    let path = root.join("tehuti/session.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    parse_enabled(&text).unwrap_or_else(|error| {
        tracing::warn!(%error, path = %path.display(), "session config ignored; restore stays off");
        false
    })
}
