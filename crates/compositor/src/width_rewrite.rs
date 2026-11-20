//! Rewriting a context file's `[window] width` to a fraction, without ever leaving a torn file.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// Replaces `path` with `contents`: a temporary file in the same directory is written and
/// synced, then renamed over the original. On any failure the original is untouched and the
/// temporary file is removed.
///
/// # Errors
/// Returns the first I/O error; nothing has replaced `path` in that case.
pub fn replace_file(path: &Path, contents: &str) -> io::Result<()> {
    let directory = path.parent().filter(|parent| !parent.as_os_str().is_empty());
    let directory = directory.unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(format!(".tehuti-{}.tmp", std::process::id()));
    let temporary = directory.join(temporary_name);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _removed = fs::remove_file(&temporary);
        return written;
    }
    if let Ok(directory) = fs::File::open(directory) {
        let _synced = directory.sync_all();
    }
    Ok(())
}

/// Rewrites the `[window] width` line of the file at `path` to `fraction`, keeping the old value
/// as a comment. `Ok(false)` when the file has no such line; the file is left as it was on error.
///
/// # Errors
/// Returns the I/O error that stopped the read or the replacement.
pub fn rewrite_file(path: &Path, fraction: (u32, u32), screen: Option<u32>) -> io::Result<bool> {
    let source = fs::read_to_string(path)?;
    tehuti_protocol::rewrite_window_width(&source, fraction, screen).map_or(Ok(false), |rewritten| {
        replace_file(path, &rewritten).map(|()| true)
    })
}
