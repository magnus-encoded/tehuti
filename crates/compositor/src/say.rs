//! Opt-in typed speech input from a FIFO in the runtime directory.

use core::cell::RefCell;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read as _},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::fs::{FileTypeExt as _, OpenOptionsExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
};

use tehuti_protocol::Text;

pub const FIFO_NAME: &str = "tehuti-say";
const MAX_LINE: usize = 64 * 1024;

/// Owns the FIFO path and a nonblocking descriptor. The path is removed on drop.
#[derive(Debug)]
pub struct SayFifo {
    file: File,
    path: PathBuf,
    state: RefCell<LineState>,
}

#[derive(Debug, Default)]
struct LineState {
    pending: Vec<u8>,
    discard_line: bool,
}

impl SayFifo {
    /// Creates a private FIFO at `runtime_directory/tehuti-say`.
    ///
    /// # Errors
    /// Returns an error if the directory or FIFO cannot be used. Existing FIFOs are replaced;
    /// other path types are preserved. The caller must hold the single-instance lock.
    pub fn create(runtime_directory: &Path) -> io::Result<Self> {
        let path = runtime_directory.join(FIFO_NAME);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_fifo() => std::fs::remove_file(&path)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "typed speech path is not a FIFO",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        rustix::fs::mkfifoat(rustix::fs::CWD, &path, rustix::fs::Mode::from_bits_retain(0o600))?;
        if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            let _removed = std::fs::remove_file(&path);
            return Err(error);
        }
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) => {
                let _removed = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        Ok(Self {
            file,
            path,
            state: RefCell::new(LineState::default()),
        })
    }

    /// Drains currently available bytes and returns complete, valid text lines.
    ///
    /// # Errors
    /// Returns an error if the FIFO read fails.
    pub fn drain(&self) -> io::Result<Vec<Text>> {
        let mut lines = Vec::new();
        let mut bytes = [0_u8; 4096];
        let mut file = &self.file;
        let mut state = self.state.borrow_mut();
        loop {
            match file.read(&mut bytes) {
                Ok(0) => break,
                Ok(size) => {
                    for &byte in bytes.iter().take(size) {
                        if byte == b'\n' {
                            if !state.discard_line {
                                let line = core::mem::take(&mut state.pending);
                                if let Ok(value) = String::from_utf8(line)
                                    && !value.trim().is_empty()
                                    && let Ok(text) = Text::new(value.trim_end_matches('\r').to_owned())
                                {
                                    lines.push(text);
                                }
                            }
                            state.pending.clear();
                            state.discard_line = false;
                        } else if !state.discard_line {
                            if state.pending.len() < MAX_LINE {
                                state.pending.push(byte);
                            } else {
                                state.pending.clear();
                                state.discard_line = true;
                            }
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(lines)
    }
}

impl AsFd for SayFifo {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl Drop for SayFifo {
    fn drop(&mut self) {
        let _removed = std::fs::remove_file(&self.path);
    }
}
