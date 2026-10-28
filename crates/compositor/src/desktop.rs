//! Desktop entry launch boundary.

/// Launches an installed desktop entry through the desktop's launcher.
///
/// # Errors
/// The desktop entry id is invalid or the launcher is unavailable.
pub fn launch(entry_id: &str, socket: &std::ffi::OsStr) -> Result<std::process::Child, String> {
    if entry_id.is_empty() || entry_id.starts_with('-') || entry_id.contains('/') || entry_id.contains('\0') {
        return Err("invalid desktop entry id".into());
    }
    crate::client_env::command(std::ffi::OsStr::new("gtk-launch"), socket)
        .arg(entry_id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())
}

/// Discovers desktop entry ids in the XDG data directories.
#[must_use]
pub fn installed_entries() -> Vec<String> {
    let mut directories = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME") {
        directories.push(std::path::PathBuf::from(home));
    } else if let Some(home) = std::env::var_os("HOME") {
        directories.push(std::path::PathBuf::from(home).join(".local/share"));
    }
    directories.extend(std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ));
    entries_in(&directories)
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

/// Desktop entries discovered at startup and refreshed by a worker.
#[derive(Debug)]
pub struct EntryCache {
    entries: Vec<String>,
    updates: std::sync::mpsc::Receiver<Vec<String>>,
    _stop: std::sync::mpsc::Sender<()>,
}

impl EntryCache {
    /// Scans once and starts a worker that refreshes every thirty seconds.
    ///
    /// # Errors
    /// The refresh worker cannot be started.
    pub fn new() -> std::io::Result<Self> {
        let entries = installed_entries();
        let (sender, updates) = std::sync::mpsc::sync_channel(1);
        let (stop, stopped) = std::sync::mpsc::channel();
        let _worker = std::thread::Builder::new()
            .name("desktop-entries".into())
            .spawn(move || {
                while matches!(
                    stopped.recv_timeout(core::time::Duration::from_secs(30)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    match sender.try_send(installed_entries()) {
                        Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
            })?;
        Ok(Self {
            entries,
            updates,
            _stop: stop,
        })
    }

    /// Accepts a completed refresh without waiting for discovery.
    pub fn poll(&mut self) {
        if let Ok(entries) = self.updates.try_recv() {
            self.entries = entries;
        }
    }

    /// Returns the cached desktop entry ids without accessing the filesystem.
    #[must_use]
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}
