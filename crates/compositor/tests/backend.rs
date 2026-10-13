//! Backend selection needs neither a display connection nor a seat.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use tehuti::backend::{Backend, select};

    #[test]
    fn a_login_session_uses_the_seat() {
        assert_eq!(select(None, None), Backend::Tty, "login sessions need DRM");
        assert_eq!(
            select(Some(OsStr::new("")), Some(OsStr::new(""))),
            Backend::Tty,
            "empty display variables are absent"
        );
    }

    #[test]
    fn either_host_display_keeps_tehuti_nested() {
        assert_eq!(
            select(Some(OsStr::new("wayland-1")), None),
            Backend::Nested,
            "Wayland host"
        );
        assert_eq!(select(None, Some(OsStr::new(":0"))), Backend::Nested, "X11 host");
        assert_eq!(
            select(Some(OsStr::new("wayland-1")), Some(OsStr::new(":0"))),
            Backend::Nested,
            "both host protocols"
        );
    }
}
