//! Desktop entry launch boundary.

/// Launches an installed desktop entry through the desktop's launcher.
///
/// # Errors
/// The desktop entry id is invalid or the launcher is unavailable.
pub fn launch(entry_id: &str, socket: &std::ffi::OsStr) -> Result<std::process::Child, String> {
    if entry_id.is_empty() || entry_id.starts_with('-') || entry_id.contains('/') || entry_id.contains('\0') {
        return Err("invalid desktop entry id".into());
    }
    std::process::Command::new("gtk-launch")
        .arg(entry_id)
        .env("WAYLAND_DISPLAY", socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())
}

/// Opens a file or directory with its default handler.
///
/// # Errors
/// The path cannot be resolved or the handler cannot be started.
pub fn open_path(path: &str, socket: &std::ffi::OsStr) -> Result<std::process::Child, String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    std::process::Command::new("xdg-open")
        .arg(absolute)
        .env("WAYLAND_DISPLAY", socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())
}

/// Discovers desktop entry ids in the XDG data directories.
#[must_use]
pub fn installed_entries() -> Vec<String> {
    entries_in(&data_directories())
}

fn data_directories() -> Vec<std::path::PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME") {
        directories.push(std::path::PathBuf::from(home));
    } else if let Some(home) = std::env::var_os("HOME") {
        directories.push(std::path::PathBuf::from(home).join(".local/share"));
    }
    directories.extend(std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ));
    directories
}

/// Discovers desktop entry ids beneath the supplied data directories.
#[must_use]
pub fn entries_in(directories: &[std::path::PathBuf]) -> Vec<String> {
    let mut entries = Vec::new();
    for directory in directories {
        collect_entries(&directory.join("applications"), "", 0, &mut entries);
    }
    entries
}

fn collect_entries(directory: &std::path::Path, prefix: &str, depth: usize, entries: &mut Vec<String>) {
    if depth >= 16 {
        return;
    }
    let Ok(children) = std::fs::read_dir(directory) else {
        return;
    };
    for child in children.flatten() {
        let Ok(kind) = child.file_type() else {
            continue;
        };
        let Some(name) = child.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let entry_id = format!("{prefix}{name}");
        if kind.is_dir() {
            collect_entries(
                &child.path(),
                &format!("{entry_id}-"),
                depth.saturating_add(1),
                entries,
            );
        } else if name.ends_with(".desktop") && child.path().is_file() {
            entries.push(entry_id);
        }
    }
}

/// Installed desktop `Name` and `GenericName` in XDG directory priority order.
#[must_use]
pub fn application_hotwords(directories: &[std::path::PathBuf]) -> Vec<(String, tehuti_protocol::Action)> {
    let mut words = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for directory in directories {
        let mut ids = entries_in(core::slice::from_ref(directory));
        ids.sort();
        for entry_id in ids {
            if !seen.insert(entry_id.clone()) {
                continue;
            }
            let Some(path) = entry_path(&directory.join("applications"), "", &entry_id, 0) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            let mut in_entry = false;
            let mut names = Vec::new();
            let mut hidden = false;
            for line in source.lines().map(str::trim) {
                if line.starts_with('[') {
                    in_entry = line == "[Desktop Entry]";
                }
                if !in_entry {
                    continue;
                }
                if matches!(line, "Hidden=true" | "NoDisplay=true") {
                    hidden = true;
                }
                if let Some(name) = line
                    .strip_prefix("Name=")
                    .or_else(|| line.strip_prefix("GenericName="))
                {
                    names.push(name.to_owned());
                }
            }
            if hidden {
                continue;
            }
            let Ok(desktop_entry) = tehuti_protocol::Text::new(entry_id) else {
                continue;
            };
            for name in names {
                if !name.trim().is_empty() {
                    words.push((
                        name,
                        tehuti_protocol::Action::Launch {
                            desktop_entry: desktop_entry.clone(),
                        },
                    ));
                }
            }
        }
    }
    words.sort_by(|left, right| left.0.cmp(&right.0));
    words
}

fn entry_path(
    directory: &std::path::Path,
    prefix: &str,
    wanted: &str,
    depth: usize,
) -> Option<std::path::PathBuf> {
    if depth >= 16 {
        return None;
    }
    let children = std::fs::read_dir(directory).ok()?;
    for child in children.flatten() {
        let Some(name) = child.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let entry_id = format!("{prefix}{name}");
        let Ok(kind) = child.file_type() else { continue };
        if kind.is_dir() {
            if let Some(path) = entry_path(
                &child.path(),
                &format!("{entry_id}-"),
                wanted,
                depth.saturating_add(1),
            ) {
                return Some(path);
            }
        } else if entry_id == wanted {
            return Some(child.path());
        }
    }
    None
}

type AppHotwords = Vec<(String, tehuti_protocol::Action)>;
type EntryUpdate = (Vec<String>, AppHotwords);

/// Desktop entries discovered at startup and refreshed by a worker.
#[derive(Debug)]
pub struct EntryCache {
    entries: Vec<String>,
    hotwords: AppHotwords,
    updates: std::sync::mpsc::Receiver<EntryUpdate>,
    _stop: std::sync::mpsc::Sender<()>,
}

impl EntryCache {
    /// Scans once and starts a worker that refreshes every thirty seconds.
    ///
    /// # Errors
    /// The refresh worker cannot be started.
    pub fn new() -> std::io::Result<Self> {
        let entries = installed_entries();
        let hotwords = application_hotwords(&data_directories());
        let (sender, updates) = std::sync::mpsc::sync_channel(1);
        let (stop, stopped) = std::sync::mpsc::channel();
        let _worker = std::thread::Builder::new()
            .name("desktop-entries".into())
            .spawn(move || {
                while matches!(
                    stopped.recv_timeout(core::time::Duration::from_secs(30)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    match sender.try_send((installed_entries(), application_hotwords(&data_directories()))) {
                        Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
            })?;
        Ok(Self {
            entries,
            hotwords,
            updates,
            _stop: stop,
        })
    }

    /// Accepts a completed refresh without waiting for discovery.
    pub fn poll(&mut self) -> bool {
        if let Ok((entries, hotwords)) = self.updates.try_recv() {
            self.entries = entries;
            self.hotwords = hotwords;
            return true;
        }
        false
    }

    #[must_use]
    pub fn hotwords(&self) -> &[(String, tehuti_protocol::Action)] {
        &self.hotwords
    }

    /// Returns the cached desktop entry ids without accessing the filesystem.
    #[must_use]
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}
