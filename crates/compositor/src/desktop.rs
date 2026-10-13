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
