//! Select the host window or the login session's seat without opening a device.

use std::ffi::OsStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Nested,
    Tty,
}

#[must_use]
pub fn select(wayland_display: Option<&OsStr>, display: Option<&OsStr>) -> Backend {
    if wayland_display.is_some_and(|value| !value.is_empty())
        || display.is_some_and(|value| !value.is_empty())
    {
        Backend::Nested
    } else {
        Backend::Tty
    }
}
