//! Loads independent context files, containing failures to each file.

use std::{
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

const MAX_CONTEXT_BYTES: u32 = 64 * 1024;

use tehuti_protocol::{Context, parse_context};

/// Loads `context.toml` in the directory and its immediate child directories.
/// Paths are sorted; the first context wins when application ids overlap.
#[must_use]
pub fn load(directory: &Path) -> Vec<Context> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(?directory, %error, "context directory unavailable");
            return Vec::new();
        }
    };
    let mut paths = vec![directory.join("context.toml")];
    for entry in entries {
        match entry {
            Ok(entry) if entry.path().is_dir() => paths.push(entry.path().join("context.toml")),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "context directory entry skipped"),
        }
    }
    paths.sort();
    let mut contexts = Vec::new();
    for path in paths {
        match read_context(&path) {
            Ok(source) => match parse_context(&source) {
                Ok(context) => contexts.push(context),
                Err(error) => tracing::warn!(?path, %error, "invalid context skipped"),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(?path, %error, "unreadable context skipped"),
        }
    }
    contexts
}

fn read_context(path: &Path) -> io::Result<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "context must be a regular file",
        ));
    }
    let mut source = String::new();
    let bytes_read = file
        .take(u64::from(MAX_CONTEXT_BYTES) + 1)
        .read_to_string(&mut source)?;
    if bytes_read > usize::try_from(MAX_CONTEXT_BYTES).unwrap_or(usize::MAX) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "context exceeds 64 KiB",
        ));
    }
    Ok(source)
}
