//! Supervised Xwayland server and X11 window management.
use crate::state::Compositor;
use core::time::Duration;
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::{
    reexports::{
        calloop::{EventLoop, RegistrationToken},
        wayland_server::DisplayHandle,
    },
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};
use std::time::Instant;

#[derive(Debug)]
pub(crate) struct State {
    pub(crate) manager: Option<X11Wm>,
    pub(crate) failed: bool,
    pub(crate) shell: XWaylandShellState,
}

impl State {
    pub(crate) fn new(display: &smithay::reexports::wayland_server::DisplayHandle) -> Self {
        Self {
            manager: None,
            failed: false,
            shell: XWaylandShellState::new::<Compositor>(display),
        }
    }
}

/// X11 events are dispatched alongside the compositor's main event loop.
pub struct Runtime {
    event_loop: EventLoop<'static, Compositor>,
    source: Option<RegistrationToken>,
    retry_at: Instant,
    export_environment: bool,
    display_name: Option<String>,
    display_number: Option<u32>,
}

impl core::fmt::Debug for Runtime {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Runtime")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// Creates the X11 event loop.
    /// # Errors
    /// The event loop's poll descriptor cannot be created.
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            event_loop: EventLoop::try_new().map_err(std::io::Error::other)?,
            source: None,
            retry_at: Instant::now(),
            export_environment: false,
            display_name: None,
            display_number: None,
        })
    }

    /// Enables publishing DISPLAY to systemd and D-Bus activation.
    pub const fn export_session_environment(&mut self) {
        self.export_environment = true;
    }

    /// The display reserved for this Xwayland server.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Dispatches X11 events and restarts a failed server with a bounded retry rate.
    pub fn dispatch(&mut self, display: &DisplayHandle, compositor: &mut Compositor) {
        if compositor.x11.failed {
            tracing::warn!("Xwayland disconnected; restarting");
            let fresh = match EventLoop::try_new() {
                Ok(event_loop) => event_loop,
                Err(error) => {
                    tracing::warn!(%error, "X11 event loop recovery failed");
                    return;
                }
            };
            compositor.clear_x11();
            self.event_loop = fresh;
            self.source = None;
            compositor.x11.failed = false;
            self.retry_at = Instant::now()
                .checked_add(Duration::from_secs(1))
                .unwrap_or_else(Instant::now);
        }
        if self.source.is_none()
            && Instant::now() >= self.retry_at
            && let Err(error) = self.start(display, compositor)
        {
            tracing::warn!(%error, "Xwayland unavailable; retrying");
            self.retry_at = Instant::now()
                .checked_add(Duration::from_secs(5))
                .unwrap_or_else(Instant::now);
        }
        if let Err(error) = self.event_loop.dispatch(Duration::ZERO, compositor) {
            tracing::warn!(%error, "X11 event dispatch failed");
            compositor.x11.failed = true;
        }
    }

    fn start(
        &mut self,
        display: &DisplayHandle,
        compositor: &mut Compositor,
    ) -> Result<(), Box<dyn core::error::Error>> {
        let (server, client) = XWayland::spawn(
            display,
            self.display_number,
            core::iter::empty::<(String, String)>(),
            true,
            std::process::Stdio::null(),
            std::process::Stdio::inherit(),
            |_| {},
        )?;
        self.display_number = Some(server.display_number());
        let display_name = format!(":{}", server.display_number());
        let handle = self.event_loop.handle();
        let wm_handle = handle.clone();
        self.source = Some(handle.insert_source(server, move |event, (), state| match event {
            XWaylandEvent::Ready { x11_socket, .. } => {
                match X11Wm::start_wm(wm_handle.clone(), x11_socket, client.clone()) {
                    Ok(manager) => {
                        state.x11.manager = Some(manager);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "X11 window manager failed");
                        state.x11.failed = true;
                    }
                }
            }
            XWaylandEvent::Error => state.x11.failed = true,
        })?);
        crate::child_environment::set_display(display_name.clone().into());
        if self.export_environment {
            crate::child_environment::export(&display_name);
        }
        self.display_name = Some(display_name);
        compositor.x11.failed = false;
        Ok(())
    }
}

/// Whether client metadata requests a floating window.
#[must_use]
pub fn should_float(
    override_redirect: bool,
    transient: bool,
    dialog: bool,
    minimum: Option<(i32, i32)>,
    maximum: Option<(i32, i32)>,
) -> bool {
    override_redirect
        || transient
        || dialog
        || minimum.is_some_and(|size| size.0 > 0 && size.1 > 0 && Some(size) == maximum)
}

#[cfg(test)]
mod tests {
    use super::should_float;

    #[test]
    fn float_rules_cover_client_metadata() {
        let cases = [
            ("normal", false, false, false, None, None, false),
            ("override redirect", true, false, false, None, None, true),
            ("transient", false, true, false, None, None, true),
            ("dialog splash utility", false, false, true, None, None, true),
            (
                "fixed",
                false,
                false,
                false,
                Some((640, 480)),
                Some((640, 480)),
                true,
            ),
            (
                "resizable",
                false,
                false,
                false,
                Some((640, 480)),
                Some((800, 600)),
                false,
            ),
            ("minimum only", false, false, false, Some((640, 480)), None, false),
            ("maximum only", false, false, false, None, Some((640, 480)), false),
            (
                "unspecified",
                false,
                false,
                false,
                Some((0, 0)),
                Some((0, 0)),
                false,
            ),
            (
                "one dimension unconstrained",
                false,
                false,
                false,
                Some((640, 0)),
                Some((640, 0)),
                false,
            ),
            (
                "combined",
                true,
                true,
                true,
                Some((640, 480)),
                Some((640, 480)),
                true,
            ),
        ];
        for (name, override_redirect, transient, dialog, minimum, maximum, expected) in cases {
            assert_eq!(
                should_float(override_redirect, transient, dialog, minimum, maximum),
                expected,
                "{name}"
            );
        }
    }
}
