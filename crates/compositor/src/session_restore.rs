//! Session Restore: which applications were open, and where, saved on a clean exit and
//! relaunched on the next start.
//!
//! The XDG state file holds desktop entry ids, window places and validated terminal session
//! identities. Restore launches installed applications and attaches existing sessions.

use core::time::Duration;
use std::os::unix::fs::OpenOptionsExt as _;
use std::{io, io::Read as _, io::Write as _, path::PathBuf, time::Instant};

use serde::{Deserialize, Serialize};
use tehuti_core::layout::{Snapshot, WindowId};

const VERSION: u32 = 1;
/// How long a launched application has to open its window before its place is forgotten.
pub const PLACEMENT_LIFETIME: Duration = Duration::from_secs(60);
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
    /// The existing persistent terminal session, when its launch command identifies one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalSession>,
}

impl SavedWindow {
    pub(crate) fn launch(&self, socket: &std::ffi::OsStr) -> Result<crate::launch::Launch, String> {
        let entry = self.app_id.strip_suffix(".desktop").unwrap_or(&self.app_id);
        let program = entry.rsplit('.').next().unwrap_or(entry).to_ascii_lowercase();
        if self.terminal.is_none() && matches!(program.as_str(), "foot" | "alacritty" | "kitty") {
            return Err("persistent terminal session unavailable".to_owned());
        }
        self.terminal.as_ref().map_or_else(
            || crate::desktop::launch_for(&self.app_id, socket),
            |terminal| terminal.launch(&self.app_id, socket),
        )
    }
}

/// A terminal emulator whose command line can identify a persistent tmux session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalEmulator {
    Foot,
    Alacritty,
    Kitty,
}

/// A tmux session to attach to. No command or replacement session is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSession {
    emulator: TerminalEmulator,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app_id: Option<String>,
}

impl TerminalSession {
    fn valid(&self) -> bool {
        let name = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && !value.starts_with('-')
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        };
        self.target.as_deref().is_none_or(name)
            && self.socket.as_deref().is_none_or(name)
            && self.app_id.as_deref().is_none_or(|app_id| {
                !app_id.is_empty() && !app_id.starts_with('-') && !app_id.contains(['/', '\0'])
            })
    }

    pub(crate) fn from_command(command: &[u8], app_id: String) -> Option<Self> {
        let arguments: Vec<_> = command
            .split(|byte| *byte == 0)
            .filter(|argument| !argument.is_empty())
            .map(core::str::from_utf8)
            .collect::<Result<_, _>>()
            .ok()?;
        let emulator = match std::path::Path::new(arguments.first()?).file_name()?.to_str()? {
            "foot" => TerminalEmulator::Foot,
            "alacritty" => TerminalEmulator::Alacritty,
            "kitty" => TerminalEmulator::Kitty,
            _ => return None,
        };
        let attachment = Self::attachment(&arguments);
        let mut session = Self {
            emulator,
            target: attachment.as_ref().map(|(target, _)| target.clone()),
            socket: attachment.and_then(|(_, socket)| socket),
            app_id: Some(app_id),
        };
        if !session.valid() {
            session.target = None;
            session.socket = None;
        }
        session.valid().then_some(session)
    }

    fn attachment(arguments: &[&str]) -> Option<(String, Option<String>)> {
        let index = arguments.iter().position(|argument| *argument == "tmux")?;
        let mut arguments = arguments.get(index.saturating_add(1)..)?.iter().copied();
        let first = arguments.next()?;
        let (socket, operation) = if first == "-L" {
            (Some(arguments.next()?.to_owned()), arguments.next()?)
        } else {
            (None, first)
        };
        if !matches!(operation, "attach" | "attach-session") || arguments.next()? != "-t" {
            return None;
        }
        let target = arguments.next()?.trim_start_matches('=').to_owned();
        if arguments.next().is_some() {
            return None;
        }
        Some((target, socket))
    }

    pub(crate) fn launch(
        &self,
        app_id: &str,
        socket: &std::ffi::OsStr,
    ) -> Result<crate::launch::Launch, String> {
        let target = self
            .target
            .as_ref()
            .ok_or_else(|| "persistent terminal session unavailable".to_owned())?;
        let (program, option) = match self.emulator {
            TerminalEmulator::Foot => ("foot", "--app-id"),
            TerminalEmulator::Alacritty => ("alacritty", "--class"),
            TerminalEmulator::Kitty => ("kitty", "--class"),
        };
        let window_app_id = self.app_id.as_deref().unwrap_or(app_id);
        let mut launch = crate::launch::Launch::new(program, app_id)
            .arg(option)
            .arg(window_app_id)
            .on_socket(socket);
        if matches!(self.emulator, TerminalEmulator::Alacritty) {
            launch = launch.arg("-e");
        }
        launch = launch.arg("tmux");
        if let Some(server) = &self.socket {
            launch = launch.arg("-L").arg(server);
        }
        Ok(launch.arg("attach-session").arg("-t").arg(format!("={target}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    version: u32,
    windows: Vec<SavedWindow>,
    /// Index into `windows` of the window that had focus; at most one, and always a real entry.
    #[serde(default)]
    focused: Option<usize>,
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
        Self::capture_with_terminals(snapshot, app_id_of, |_| None, installed)
    }

    pub(crate) fn capture_with_terminals(
        snapshot: &Snapshot,
        app_id_of: impl Fn(WindowId) -> Option<String>,
        terminal_of: impl Fn(WindowId) -> Option<TerminalSession>,
        installed: &[String],
    ) -> Self {
        let mut windows = Vec::new();
        let workspaces = snapshot
            .outputs
            .first()
            .map(|output| output.workspaces.as_slice());
        let mut rank = 0;
        let mut focused = None;
        for workspace in workspaces.unwrap_or_default() {
            let before = windows.len();
            for (column, view) in workspace.columns.iter().enumerate() {
                for tile in &view.tiles {
                    if windows.len() < MAX_WINDOWS
                        && let Some(app_id) = app_id_of(tile.window).and_then(|app_id| {
                            installed
                                .iter()
                                .find(|entry| {
                                    *entry == &app_id
                                        || entry.strip_suffix(".desktop") == Some(app_id.as_str())
                                })
                                .cloned()
                        })
                    {
                        if snapshot.focused_window == Some(tile.window) {
                            focused = Some(windows.len());
                        }
                        windows.push(SavedWindow {
                            app_id,
                            workspace: rank,
                            column,
                            terminal: terminal_of(tile.window),
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
            focused,
        }
    }

    /// The saved window that had focus.
    #[must_use]
    pub fn focused(&self) -> Option<&SavedWindow> {
        self.windows.get(self.focused?)
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
        if saved.focused.is_some_and(|index| index >= saved.windows.len()) {
            return Err(LoadError::Invalid("focused window is not in the list".into()));
        }
        let plain = |entry: &str| {
            !entry.is_empty() && !entry.starts_with('-') && !entry.contains('/') && !entry.contains('\0')
        };
        if saved.windows.iter().any(|window| !plain(&window.app_id)) {
            return Err(LoadError::Invalid("invalid desktop entry id".into()));
        }
        if saved
            .windows
            .iter()
            .filter_map(|window| window.terminal.as_ref())
            .any(|terminal| !terminal.valid())
        {
            return Err(LoadError::Invalid("invalid terminal session identity".into()));
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

    /// An empty session is not written, so it never replaces an earlier good one.
    ///
    /// # Errors
    /// Directory creation, write or atomic replacement failed.
    pub fn save(&self, saved: &Saved) -> io::Result<()> {
        if saved.windows.is_empty() {
            return Ok(());
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("state directory unavailable"))?;
        std::fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("tmp");
        let _removed = std::fs::remove_file(&temporary);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
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
        let _read_bytes = read.map_err(LoadError::Unreadable)?;
        Saved::parse(&text).map(Some)
    }

    fn discard(&self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(%error, "session file not removed");
        }
    }
}

/// Where each launched application's windows still have to go. A window that opens claims
/// the entry whose launch token it carries; entries nobody claims expire. `K` names an
/// opened window.
#[derive(Debug)]
pub struct Placements<K> {
    pending: Vec<(usize, SavedWindow, Instant)>,
    /// The first window opened in each saved column, which later tiles of it stack under.
    anchors: Vec<((usize, usize), K)>,
    focus: FocusRestore<K>,
    /// The focus the restore left, from the moment it began; any other focus is the user's
    /// choice. `None` is the state of no window focused.
    expected_focus: Option<K>,
    /// The tally of the restore until it is reported; `None` once given out.
    tally: Option<Tally>,
}

#[derive(Debug, Default)]
struct Tally {
    restored: usize,
    failed: Vec<String>,
}

/// How a finished restore went: the windows that opened and the applications that did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Saved windows that opened and were placed.
    pub restored: usize,
    /// App ids of the saved windows that never opened: skipped, failed to launch or expired.
    pub failed: Vec<String>,
}

/// The pending return of the prior focus. A user focus change ends it, so a restore cannot
/// override the user.
#[derive(Debug)]
enum FocusRestore<K> {
    /// Nothing to restore, or the user took over.
    Off,
    /// The saved window has not opened yet; its launch token.
    Awaiting(usize),
    /// The saved window is open; it gets focus once every launch has opened or expired.
    Armed(K),
}

/// Where a claimed window goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim<K> {
    /// Into a new column at the saved place.
    Column {
        /// Workspace position among the non-empty workspaces.
        workspace: usize,
        /// Column position inside that workspace.
        column: usize,
    },
    /// Under the window that opened first in the same saved column. The saved column is the
    /// fallback if that window has closed by then.
    Under {
        /// The window that opened first in the saved column.
        anchor: K,
        /// Workspace position among the non-empty workspaces.
        workspace: usize,
        /// Column position inside that workspace.
        column: usize,
    },
}

/// The environment variable that carries a restore launch's token (the saved window's
/// index) into the launched process and everything it starts.
pub const TOKEN_VARIABLE: &str = "TEHUTI_RESTORE_TOKEN";

/// The most of a process environment read when looking for a launch token.
pub const MAX_ENVIRON_BYTES: u64 = 128 * 1024;

/// The launch token in an environment stream, reading at most [`MAX_ENVIRON_BYTES`].
pub fn token_from_reader(reader: impl io::Read) -> Option<usize> {
    let mut environ = Vec::new();
    let _read = reader.take(MAX_ENVIRON_BYTES).read_to_end(&mut environ).ok()?;
    token_from_environment(&environ)
}

/// The launch token in the raw bytes of `/proc/<pid>/environ`, if the process has one.
#[must_use]
pub fn token_from_environment(environ: &[u8]) -> Option<usize> {
    let prefix = format!("{TOKEN_VARIABLE}=");
    environ
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .and_then(|value| core::str::from_utf8(value).ok())
        .and_then(|value| value.parse().ok())
}

impl<K: Clone + PartialEq> Placements<K> {
    /// Ends the pending focus restore if focus is not where the restore left it: the user
    /// moved it.
    pub fn cancel_focus_if_moved(&mut self, current: Option<&K>) {
        if self.expected_focus.as_ref() != current {
            self.focus = FocusRestore::Off;
        }
    }
}

impl<K: Clone> Placements<K> {
    #[must_use]
    pub fn new(saved: &Saved, now: Instant, focus_at_start: Option<&K>) -> Self {
        let expires = now.checked_add(PLACEMENT_LIFETIME).unwrap_or(now);
        Self {
            pending: saved
                .windows
                .iter()
                .enumerate()
                .map(|(token, window)| (token, window.clone(), expires))
                .collect(),
            anchors: Vec::new(),
            focus: saved.focused.map_or(FocusRestore::Off, FocusRestore::Awaiting),
            expected_focus: focus_at_start.cloned(),
            tally: Some(Tally::default()),
        }
    }

    /// Starts a restore: `launch` is called for each saved window with its token and says
    /// whether it started; the windows that did not start are failed at once.
    pub fn begin(
        saved: &Saved,
        now: Instant,
        focus_at_start: Option<&K>,
        mut launch: impl FnMut(usize, &SavedWindow) -> bool,
    ) -> Self {
        let mut placements = Self::new(saved, now, focus_at_start);
        for (token, window) in saved.windows.iter().enumerate() {
            if !launch(token, window) {
                placements.launch_failed(token);
            }
        }
        placements
    }

    /// Moves every entry whose time has run out from pending to failed.
    fn expire(&mut self, now: Instant) {
        let (live, expired): (Vec<_>, Vec<_>) = core::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(_, _, expires)| now < *expires);
        self.pending = live;
        if let Some(tally) = self.tally.as_mut() {
            tally
                .failed
                .extend(expired.into_iter().map(|(_, saved, _)| saved.app_id));
        }
    }

    /// Records that the launch with this token did not start, so its entry stops waiting.
    pub fn launch_failed(&mut self, token: usize) {
        if let Some(index) = self.pending.iter().position(|(wanted, _, _)| *wanted == token) {
            let (_, saved, _) = self.pending.remove(index);
            if let Some(tally) = self.tally.as_mut() {
                tally.failed.push(saved.app_id);
            }
        }
    }

    /// The outcome once every launch has opened or failed or expired. Given out once.
    pub fn report(&mut self, now: Instant) -> Option<Report> {
        self.expire(now);
        if !self.pending.is_empty() {
            return None;
        }
        self.tally.take().map(|tally| Report {
            restored: tally.restored,
            failed: tally.failed,
        })
    }

    /// The saved place for a window that just opened, removed from the list. The window
    /// proves it was launched by the restore with the token its process carries; a window
    /// without one, or whose entry expired, opens as usual.
    pub fn claim(&mut self, token: Option<usize>, now: Instant, window: &K) -> Option<Claim<K>> {
        self.expire(now);
        let token = token?;
        let index = self.pending.iter().position(|(wanted, _, _)| *wanted == token)?;
        let (_, saved, _) = self.pending.remove(index);
        if let Some(tally) = self.tally.as_mut() {
            tally.restored = tally.restored.saturating_add(1);
        }
        let (workspace, column) = (saved.workspace, saved.column);
        let group = (workspace, column);
        if matches!(self.focus, FocusRestore::Awaiting(wanted) if wanted == token) {
            self.focus = FocusRestore::Armed(window.clone());
        }
        if let Some((_, anchor)) = self.anchors.iter().find(|(candidate, _)| *candidate == group) {
            return Some(Claim::Under {
                anchor: anchor.clone(),
                workspace,
                column,
            });
        }
        self.anchors.push((group, window.clone()));
        Some(Claim::Column { workspace, column })
    }

    /// Records the focus the restore just caused.
    pub fn note_focus(&mut self, current: Option<&K>) {
        self.expected_focus = current.cloned();
    }

    /// The window that had focus before the exit, once every launch has opened or expired,
    /// so a later window cannot take focus back. Given out once.
    pub fn settled_focus(&mut self, now: Instant) -> Option<K> {
        self.expire(now);
        if !self.pending.is_empty() {
            return None;
        }
        match core::mem::replace(&mut self.focus, FocusRestore::Off) {
            FocusRestore::Armed(window) => Some(window),
            FocusRestore::Off | FocusRestore::Awaiting(_) => None,
        }
    }
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.tally.is_none() && !matches!(self.focus, FocusRestore::Armed(_))
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
