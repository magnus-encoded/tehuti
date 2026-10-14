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
select another user binary and context directory. A start failure starts Plasma; the reason is in `~/.local/state/tehuti/session.log`.

Hold Guide for three seconds to open the system menu. Log out stops Tehuti with
exit status 21; Restart Tehuti stops it with exit status 20. Start follows the
app mapping and never counts toward the hold. Plasma logout returns to Tehuti. Bind
`dist/tehuti-switch` to a KDE shortcut or controller button; it calls
`qdbus org.kde.Shutdown /Shutdown logout`. If qdbus is missing, use Plasma's
built-in Log Out action. Ctrl+Alt+Backspace ends the login (exit status 21).
Ctrl+Alt+F1 through Ctrl+Alt+F12 switch VTs through libseat.

Phone SSH `pkill -TERM tehuti`, start failures and crashes also start Plasma.
Three failures within 60 seconds keep the session in Plasma. A non-zero Plasma
exit restarts Plasma after a delay; a successful logout clears the failure
count and retries Tehuti. Missing `startplasma-wayland` logs a message and retries Tehuti
with a delay. Children receive SIGTERM, then SIGKILL after a 200 ms grace period.

Without a host display, libseat opens the seat's primary GPU and DRM uses the
first connected output and its first advertised mode. Pixman renders clients
and notices into two linear scanout buffers. This path has no GPU acceleration;
clients need shared-memory or CPU-mappable buffers. Nested operation uses winit.
Seat suspension stops rendering and releases the controller; activation resumes
rendering and discovery. A change to the selected DRM device switches to Plasma. Extra outputs are unused.

Hardware verification: log in to Tehuti, open and control an application, check
notices, switch away and back to the VT, switch to Plasma, and log out of Plasma
to return to Tehuti. Check the
log for DRM and controller errors. These checks require the owner's hardware.
