# Tehuti login session

## Installing from a release

Run `scripts/install-release` for the latest GitHub release, or
`scripts/install-release v1.0.0` for a tag. It supports x86_64 and i686 Linux,
verifies SHA256SUMS, and installs without building. Install gh or curl, Python 3
with venv support, tar, and sha256sum first. The host also needs the runtime
libraries for Wayland, xkbcommon, EGL/GLES, udev, libseat, libinput, GBM, DRM,
and Pixman. Releases are built on Ubuntu 22.04 and require glibc 2.35 or newer.

Each release lives in `~/.local/lib/tehuti/builds/<UTC timestamp>-<shortsha>/`,
with `tehuti`, `sidecars/`, `dist/`, and the full commit SHA in `VERSION`.
`~/.local/lib/tehuti/tehuti` points to its binary. The installer creates
`~/.local/bin/tehuti` and `tehuti-speech` launchers; speech follows the active
binary's sidecars. Existing builds stay available. The printed rollback command
atomically re-points the binary link. Installation and rollback leave running
sessions alone; the next session uses the selected build.

Speech uses the requirements stamp and venv from `scripts/try`, including
`TEHUTI_SPEECH_VENV`. Set `TEHUTI_NO_SPEECH=1` to install without speech dependencies.
The speech sidecar picks CUDA when it can run there and falls back to the CPU. The faster-whisper stack
requires a supported CTranslate2 Python distribution; its [published Linux wheels](https://opennmt.net/CTranslate2/installation)
cover x86_64 and AArch64, so use `TEHUTI_NO_SPEECH=1` on i686 unless you provide that dependency.
Session registration remains a separate owner action described below.

## Building locally

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

RESTART TEHUTI exits cleanly with status 75. The session launcher immediately
starts the compositor again with the same binary, contexts and arguments.

Menu commands come from `$XDG_CONFIG_HOME/tehuti/menu.toml` (or
`$HOME/.config/tehuti/menu.toml`). `TEHUTI_MENU_CONFIG` overrides the file path.
The file uses TOML, matching context.toml. Each entry is an argument list,
executed without a shell. Missing or empty
entries stay visible but report unavailable when selected. No session or power
commands run by default; RESTART TEHUTI is built in. `examples/menu.toml` shows neutral placeholder session
helpers and a task monitor in a terminal; replace them with commands appropriate
for the installed session. The `plasma` helper must arrange the desktop switch.

The settings view embeds the vendored Liberation Sans font from
`crates/compositor/assets/LiberationSans-Regular.ttf`, licensed under the
SIL Open Font License (OFL) 1.1. The licence and copyright notices are in
`crates/compositor/assets/Liberation-LICENSE`.
