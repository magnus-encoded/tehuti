# Tehuti

A desktop environment you drive with a game controller and your voice.

## Try it

Run `scripts/try` from a terminal in your desktop. It builds Tehuti, sets up the
speech venv on first run (in `~/.cache/tehuti-speech-venv`), then opens a window
with a foot terminal inside. Tehuti takes the controller exclusively while it runs.

**Switch:** hold Guide (the home button) and Start together for three seconds.
This releases the controller. In the Tehuti login session, it starts Plasma in
the same login. Logging out of Plasma starts Tehuti again. Bind
`dist/tehuti-switch` to a KDE shortcut or controller button to do this. It uses
`qdbus org.kde.Shutdown /Shutdown logout`; if qdbus is missing, bind Plasma's
built-in Log Out action instead.

Ctrl+Alt+Backspace in Tehuti ends the login session. Start failures and crashes
open Plasma too, as does phone SSH `pkill -TERM tehuti`. Three Tehuti failures
within 60 seconds keep the session in Plasma, including after Plasma exits.
If `startplasma-wayland` is missing from PATH, the launcher logs the problem
and retries Tehuti with a delay.

Speech: hold Share to record, release to transcribe. Set `TEHUTI_NO_SPEECH=1` to skip it.

`examples/foot/context.toml` (foot terminal):

- South: Enter. East: Backspace. West: Space. Select: Escape.
- D-pad: arrow keys. North: types `ls -la`. Start: shows a notice.
- Say "Hello" to type `echo hello from Tehuti`, "Enter" for Enter, "Clear" to type `clear`.
  Matching ignores case and punctuation (Unicode included), folds typographic apostrophes
  to `'` and compares NFC-normalized text, so "Hello." and "hello" are the same phrase.

`examples/plugins/east-to-tab` remaps East to Tab, only for apps with app id `test`.
Try it with `target/release/tehuti examples foot -- bash --norc --noprofile` (a plain shell keeps the demo predictable).
