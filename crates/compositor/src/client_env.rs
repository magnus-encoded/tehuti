//! Environment for processes connecting to this compositor.

use std::{ffi::OsStr, process::Command};

/// Prepares a client command with the compositor socket.
#[must_use]
pub fn command(program: &OsStr, socket: &OsStr) -> Command {
    let mut command = crate::child_process::command(program);
    let _command = command.env("WAYLAND_DISPLAY", socket);
    command
}
