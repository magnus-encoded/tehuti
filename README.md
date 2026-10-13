# Tehuti

A desktop environment you drive with a game controller and your voice.

## Try it

Run `scripts/try` from a terminal in your desktop. It builds Tehuti, sets up the
speech venv on first run (in `~/.cache/tehuti-speech-venv`), then opens a window
with a foot terminal inside. Tehuti takes the controller exclusively while it runs.

**Quit:** hold Guide (the home button) for three seconds. This releases the controller.

Speech: hold Share to record, release to transcribe. Set `TEHUTI_NO_SPEECH=1` to skip it.

`examples/foot/context.toml` (foot terminal):

- South: Enter. East: Backspace. West: Space. Select: Escape.
- D-pad: arrow keys. North: types `ls -la`. Start: shows a notice.
- Say "Hello" to type `echo hello from Tehuti`, "Enter" for Enter, "Clear" to type `clear`.
  Phrases must match Whisper's output exactly, including the final period.

`examples/plugins/east-to-tab` remaps East to Tab, only for apps with app id `test`.
Try it with `target/release/tehuti examples foot --app-id=test`.
