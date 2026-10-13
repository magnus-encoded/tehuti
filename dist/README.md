# Tehuti login session

Build with `cargo build --release -p tehuti`. The build needs the libseat,
libudev, libinput and Pixman development libraries, alongside the nested
backend dependencies. Put `target/release/tehuti` at `~/.local/bin/tehuti`.
Contexts and plugins live at `~/.config/tehuti` by default. The session can start
without an initial application; the controller's desktop launcher opens apps.

The owner runs `sudo scripts/install-session` from this checkout to copy only
`dist/tehuti-session` and `dist/tehuti.desktop`. Tehuti then appears among Wayland
sessions. Plasma remains selectable. The installer refuses to overwrite files.
It prints the two paths to remove. No compositor binary is installed by it.

The launcher logs to `~/.local/state/tehuti/session.log`, or beneath
`XDG_STATE_HOME` when set. `TEHUTI_BINARY` and `TEHUTI_CONTEXT_DIRECTORY` can
select another user binary and context directory. A start failure returns to the
login screen; the reason is in `~/.local/state/tehuti/session.log`.

Hold the controller's Guide button for three seconds to stop and return to the
login screen. With a keyboard, Ctrl+Alt+F1 through Ctrl+Alt+F12 switch VTs through
libseat; Ctrl+Alt+Backspace stops the compositor. From phone SSH, run
`pkill -TERM tehuti` to return to the login screen. TERM and INT stop the loop
cleanly. Children receive SIGTERM, then SIGKILL after a 200 ms grace period.

Without a host display, libseat opens the seat's primary GPU and DRM uses the
first connected output and its first advertised mode. Pixman renders clients
and notices into two linear scanout buffers. This path has no GPU acceleration;
clients need shared-memory or CPU-mappable buffers. Nested operation uses winit.
Seat suspension stops rendering and releases the controller; activation resumes
rendering and discovery. A change to the selected DRM device ends the session,
so reconnecting a display requires logging in again. Extra outputs are unused.

Hardware verification: log in to Tehuti, open and control an application, check
notices, switch away and back to the VT, quit, and log in to Plasma. Check the
log for DRM and controller errors. These checks require the owner's hardware.
