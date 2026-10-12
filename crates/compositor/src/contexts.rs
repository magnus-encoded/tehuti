//! Loads independent context files, containing failures to each file.

use std::{io, path::Path};

use tehuti_protocol::{Context, parse_context};

/// Loads `context.toml` in the directory and its immediate child directories.
/// Paths are sorted; the first context wins when application ids overlap.
///
/// # Errors
/// The context directory cannot be listed.
pub fn load(directory: &Path) -> io::Result<Vec<Context>> {
    let mut paths = vec![directory.join("context.toml")];
    for entry in std::fs::read_dir(directory)? {
        match entry {
            Ok(entry) if entry.path().is_dir() => paths.push(entry.path().join("context.toml")),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "context directory entry skipped"),
        }
    }
    paths.sort();
    let mut contexts = Vec::new();
    for path in paths {
        match std::fs::read_to_string(&path) {
            Ok(source) => match parse_context(&source) {
                Ok(context) => contexts.push(context),
                Err(error) => tracing::warn!(?path, %error, "invalid context skipped"),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(?path, %error, "unreadable context skipped"),
        }
    }
    Ok(contexts)
}
