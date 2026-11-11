# Tehuti

<img src="assets/tehuti.png" alt="Tehuti icon: an ibis head with a moon disc" width="160">

A desktop environment you drive with a game controller and your voice.

Tehuti is a Wayland compositor and shell. Windows sit in a scrolling row of
columns. A controller moves between them, opens apps and types on an on-screen
keyboard. Hold a button and speak to launch an app, run a command or dictate
text. Speech is transcribed on your machine.

![The launcher open beside a terminal window](https://github.com/magnus-encoded/tehuti/releases/download/v0.114.0/launcher.png)

## Status

Tehuti is v0 ("Apteribis"), published as GitHub pre-releases. The author uses it
as a daily desktop, but it is early. Expect rough edges, changing settings and
breaking changes to contexts and plugins. Bug reports are welcome as
[issues](https://github.com/magnus-encoded/tehuti/issues).

## What it does

- **Scrolling columns.** Windows open as columns that scroll sideways. Shoulder
  buttons and the d-pad move focus; the right stick scrolls the focused window.
- **Voice bar.** A bar along the bottom edge shows the focused app, live
  dictation, command results and a clock.
- **Launcher.** Say or type what to open. Results are ranked from your desktop
  entries and your own names for them.
- **On-screen keyboard.** Opens from a text field or by voice, edits with the
  pad, and dictates into its draft.
- **System menu and settings.** Guide opens the menu. Settings cover controller,
  microphone, bar font and wallpapers, all reachable from the pad.
- **Contexts.** A `context.toml` per application maps buttons and spoken
  phrases to keys, text and commands. Contexts reload when you save them.
- **Plugins.** Programs in any language speak JSON-RPC over stdio. See
  [the protocol](docs/protocol/README.md).
- **X11 apps** run through `xwayland-satellite` when it is installed.
- **Rendering.** GPU (GLES) on the login session, with an automatic CPU
  fallback, or nested in a window on your current desktop.

## Install

Tehuti runs on Linux. There is no distribution package yet. You get it from a
release build or from source; both start with a clone:

```sh
git clone https://github.com/magnus-encoded/tehuti
cd tehuti
```

**From a release build (no Rust needed).** The releases are pre-releases, and
the installer's default "latest" skips pre-releases, so name a tag from the
[releases page](https://github.com/magnus-encoded/tehuti/releases):

```sh
scripts/install-release v0.123.0
```

It checks SHA256SUMS, puts the binary under `~/.local/lib/tehuti`, adds
`tehuti` and `tehuti-speech` to `~/.local/bin` and sets up the speech venv.
It needs `gh` or `curl`, Python 3 with venv support, `tar` and `sha256sum`, and
the runtime libraries listed in [dist/README.md](dist/README.md). Builds exist
for x86_64 and i686 Linux and need glibc 2.35 or newer.

**From source.** You need a Rust toolchain, Python 3 with venv and pip, `foot`,
and the development libraries for xkbcommon, Wayland, EGL, udev, libseat,
libinput, GBM, DRM and Pixman. `scripts/try` builds Tehuti, sets up the speech
venv on first run and opens a window with a foot terminal inside.

## Try it

From a clone, `scripts/try` runs Tehuti nested in a window on your current
desktop, with the contexts in `examples/`. Tehuti takes the controller
exclusively while it runs; hold Guide for three seconds to quit. The nested
window needs read access to `/dev/input`, usually through the `input` group.

With a release install, run `tehuti examples foot` from the clone instead.
The command is `tehuti <context-directory> <app> [arguments...]`; it starts
`<app>` as the first window.

Check your own contexts without starting anything:

```sh
tehuti --check-config ~/.config/tehuti
```

It prints `N files checked, E errors, W warnings` and each problem. Only errors
fail the check (exit 1); warnings are advice, and a config with many speech
phrases can print a lot of them. Give the
subcommand with the dashes: `tehuti check-config` is read as a context
directory.

## Run as a session

To pick Tehuti at your login screen, run `sudo scripts/install-session` from the
clone. It copies two files, refuses to overwrite and prints the paths to remove.
Then choose "Tehuti" among the Wayland sessions. The session starts with no
application; open apps from the launcher. Contexts and plugins are read from
`~/.config/tehuti`. The launcher writes `~/.local/state/tehuti/session.log`.

## Controller

Tehuti reads pads that Linux exposes as a gamepad in `/dev/input` (wired or
Bluetooth). It watches the directory, so a pad plugged in later is picked up.
Your user needs read access to the device. Settings let you choose the
controller from the pad.

## Speech

Hold Share to record, release to transcribe. Transcription runs locally with
Whisper through a Python sidecar in the venv that `scripts/try` or
`scripts/install-release` creates. The first transcription downloads the model
(`small` by default, a few hundred megabytes; change it with
`TEHUTI_SPEECH_MODEL`). The sidecar uses a GPU when CUDA works and the CPU
otherwise; `TEHUTI_SPEECH_DEVICE=cpu` forces the CPU. Set `TEHUTI_NO_SPEECH=1` to
skip speech and its dependencies.

The voice bar along the bottom shows what was heard and what Tehuti did with
it. Matching a phrase is exact after normalising, so list each likely
mishearing as its own phrase. A context can also show its buttons and phrases
as a scrolling hint line on the bar; see
[docs/contexts.md](docs/contexts.md#mapping-ticker).

**Emergency escape:** hold Guide (the home button) and Start together for three seconds.
Start alone follows the app mapping. The hold counts only while both buttons
are down. The emergency escape releases the controller. In the Tehuti login
session, it starts Plasma in the same login. Logging out of Plasma starts Tehuti again. Bind
`dist/tehuti-switch` to a KDE shortcut or controller button to do this. It uses
`qdbus org.kde.Shutdown /Shutdown logout`; if qdbus is missing, bind Plasma's
built-in Log Out action instead.

Ctrl+Alt+Backspace in Tehuti ends the login session. Start failures and crashes
open Plasma too, as does phone SSH `pkill -TERM tehuti`. Three Tehuti failures
within 60 seconds keep the session in Plasma until a successful Plasma logout
clears the failure count and retries Tehuti.
If `startplasma-wayland` is missing from PATH, the launcher logs the problem
and retries Tehuti with a delay.

More on the login session is in [dist/README.md](dist/README.md).

## Scale

`TEHUTI_SCALE` sets the integer output scale, for example `TEHUTI_SCALE=2`.
Without it, modes wider than 2560 pixels use 2 and others use 1. Invalid values
(0, negative or not a whole number) log a warning and use that default. The
nested window honours it too.

## Contexts

A context is one `context.toml` per application. Tehuti reads contexts from the
directory you pass on the command line, and the session reads
`~/.config/tehuti`. Each application gets a folder with its own `context.toml`;
saving a file reloads it. The full format is in [docs/contexts.md](docs/contexts.md).

`examples/foot/context.toml` maps a foot terminal:

- South: Enter. East: Backspace. West: Space. Select: Escape.
- D-pad: arrow keys. North: types `ls -la`. Start: shows a notice.
- Say "Hello" to type `echo hello from Tehuti`, "Enter" for Enter, "Clear" to type `clear`.
  Matching ignores case and punctuation (Unicode included), folds typographic apostrophes
  to `'` and compares NFC-normalized text, so "Hello." and "hello" are the same phrase.

`examples/plugins/east-to-tab` remaps East to Tab, only for apps with app id `test`.
Plugins are found in `<context-directory>/plugins/`, so copy it there to try it.
Run `tehuti examples foot -- bash --norc --noprofile` to see the foot context.

### Your first context: mpv

1. Make a folder and copy the example:
   `mkdir -p ~/.config/tehuti/mpv && cp examples/mpv/context.toml ~/.config/tehuti/mpv/`
2. Check it: `tehuti --check-config ~/.config/tehuti`.
3. Start Tehuti with that directory and mpv, for example
   `tehuti ~/.config/tehuti mpv some-video.mkv`.
4. West pauses, the D-pad seeks and changes the volume, North toggles
   fullscreen. Say "pause", "forward" or "louder".
5. Edit the file and save. The change applies without a restart. If Tehuti
   shows a notice naming the file, the file has an error; the log has the rest.

A button is a name such as `west`, `dpad_left`, `left_bumper` or
`right_trigger`; the full list and the keys you can send are in
[docs/contexts.md](docs/contexts.md). To send a letter or digit that is not
one of those keys, use `text = "f"`. If the app has an id other than the one
you wrote, the context does not apply; the voice bar's left chip shows the
focused app id.

## Contributing and licence

To share a context, open a pull request that adds `examples/<app>/context.toml`.

Run `scripts/check` before every commit; CI runs the same gate. Review rules are
in [CODING_STANDARDS.md](CODING_STANDARDS.md), and each crate has a
`GLOSSARY.md` listed in [GLOSSARY-MAP.md](GLOSSARY-MAP.md). Design decisions are
recorded in [docs/adr](docs/adr). See [CONTRIBUTING.md](CONTRIBUTING.md) for
how contributions are handled.

### Licence

Licensing is decided in [ADR 0004](docs/adr/0004-licensing.md); the texts are in
[LICENSES](LICENSES) and the file map is in [LICENSE](LICENSE).

- The core and compositor are under MPL-2.0: changes to Tehuti's files must be shared.
- The protocol, its schemas and the examples are under Apache-2.0, so vendors may copy them into closed plugins.
- Docs take the licence of the part they describe.
- Plugins are separate programs and may use any licence.
- The bundled Liberation fonts keep their own licence, in `crates/compositor/assets/Liberation-LICENSE`.
