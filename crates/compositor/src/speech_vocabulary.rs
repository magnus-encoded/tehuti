//! Local names and loaded phrases offered to the speech engine.
use std::{
    io::Read as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};
use tehuti_protocol::Context;

/// Reads local names using the session's XDG directories.
#[must_use]
pub fn load(contexts: &[Context]) -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let data_home =
        std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from);
    let mut data = vec![data_home];
    data.extend(std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ));
    read(
        contexts,
        &config.join("tehuti"),
        &home.join("Projects"),
        &[home.join(".agents/skills"), home.join(".claude/skills")],
        &data,
    )
}

/// Reads names from supplied directories. Missing or unreadable sources contribute nothing.
#[must_use]
pub fn read(
    contexts: &[Context],
    config: &Path,
    projects: &Path,
    skills: &[PathBuf],
    data: &[PathBuf],
) -> Vec<String> {
    let mut words: Vec<String> = read_text(&config.join("vocabulary.txt"))
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    words.extend(directory_names(projects));
    for directory in skills {
        words.extend(directory_names(directory));
    }
    let mut phrases: Vec<String> = contexts
        .iter()
        .flat_map(Context::speech_mappings)
        .map(|mapping| mapping.phrase.to_owned())
        .collect();
    phrases.sort();
    words.extend(phrases);
    for directory in data {
        application_names(&directory.join("applications"), 0, &mut words);
    }
    bounded(words)
}

/// Normalizes and caps a vocabulary below the speech transport limit.
#[must_use]
pub fn bounded(words: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0usize;
    words
        .into_iter()
        .map(|word| word.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|word| {
            let cost = serde_json::to_vec(word).map_or(usize::MAX, |encoded| encoded.len().saturating_add(1));
            if word.is_empty()
                || word.contains('\0')
                || word.len() > 256
                || bytes.saturating_add(cost) > 30_000
                || !seen.insert(word.to_lowercase())
            {
                return false;
            }
            bytes = bytes.saturating_add(cost);
            true
        })
        .collect()
}

/// Immediate folder names in stable order, excluding files.
#[must_use]
pub fn directory_names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

pub(crate) fn read_text(path: &Path) -> String {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    else {
        return String::new();
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return String::new();
    }
    let mut text = String::new();
    if file.take(65536).read_to_string(&mut text).is_err() {
        return String::new();
    }
    text
}

fn application_names(directory: &Path, depth: usize, words: &mut Vec<String>) {
    if depth >= 16 {
        return;
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            application_names(&path, depth.saturating_add(1), words);
        } else if path.extension().is_some_and(|extension| extension == "desktop") {
            let mut in_entry = false;
            for line in read_text(&path).lines().map(str::trim) {
                if line.starts_with('[') {
                    in_entry = line == "[Desktop Entry]";
                } else if in_entry && let Some(("Name" | "GenericName", value)) = line.split_once('=') {
                    words.push(
                        value
                            .replace("\\s", " ")
                            .replace("\\n", " ")
                            .replace("\\t", " ")
                            .replace("\\\\", "\\"),
                    );
                }
            }
        }
    }
}

/// Installs project names and paths for each context's `<project>` template.
pub fn configure_projects(contexts: &mut [Context], directory: &Path) {
    let projects: Vec<_> = directory_names(directory)
        .into_iter()
        .filter_map(|name| {
            let path = directory.join(&name).to_str()?.to_owned();
            Some((name, path))
        })
        .collect();
    for context in contexts {
        context.set_projects(projects.clone());
    }
}

/// Installs project names from the session's home directory.
pub fn configure_session_projects(contexts: &mut [Context]) {
    if let Some(home) = std::env::var_os("HOME") {
        configure_projects(contexts, &PathBuf::from(home).join("Projects"));
    }
}
