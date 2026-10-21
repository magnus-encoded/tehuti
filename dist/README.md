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

For emergency escape, hold Guide+Start together for three seconds to switch to
Plasma in the same login (Tehuti exit status 20). Start alone follows the app mapping; the hold
counts only while both buttons are down. Plasma logout returns to Tehuti. Bind
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

Hold Guide for half a second to open the system menu in any controller mode.
A shorter tap keeps switching the App/Columns focus level. Use the D-pad to
select an entry, A to select, and B or Guide to close. Session and power actions
ask for a second A press; release A between presses. Guide+Start held together
for three seconds remains the emergency escape while the menu is open.

Menu commands come from `$XDG_CONFIG_HOME/tehuti/menu.json` (or
`$HOME/.config/tehuti/menu.json`). `TEHUTI_MENU_CONFIG` overrides the file path.
Each entry is an argument list, executed without a shell. Missing or empty
entries stay visible but report unavailable when selected. No session or power
commands run by default. `examples/menu.json` shows neutral placeholder session
helpers and a task monitor in a terminal; replace them with commands appropriate
for the installed session. The `plasma` helper must arrange the desktop switch.
