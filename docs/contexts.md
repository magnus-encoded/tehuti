# Declarative contexts

Start with `tehuti <context-directory> <application> [arguments...]`.
The directory can contain a `context.toml` and child directories with their own
`context.toml`. Files load in path order. The first exact application-id match
selects the active context. Invalid files are logged and skipped.

```toml
app_id = "org.example.App"

[buttons.east]
key = "tab"

[buttons.dpad_up]
key = "up"
```

Button names use snake case, such as `south`, `left_bumper` and `dpad_left`.
Keys are `enter`, `escape`, `space`, `tab`, `backspace`, `up`, `down`, `left`,
`right`, `home`, `end`, `page_up` and `page_down`. A mapping sends a complete key tap on button press. Releases send
nothing. Unknown fields, buttons and keys are rejected. Application ids must
contain a non-whitespace character.

The global layer consumes South presses and sends Enter. The active context
handles other mapped presses. The fallback discards events that neither handles.
Focus changes select a context by the focused toplevel's current application id.

The escape chord consumes every Guide press and release before context routing.
A context mapping on Guide never fires. Holding Guide for three seconds emits
Quit once; releasing it before then cancels the hold.

Double tapping View (Select) within 300 ms toggles mouse mode. A single
press sends its normal mapping on release. Left stick moves the pointer with
more speed at larger deflections; right stick scrolls. South (A) holds the left
mouse button for dragging, East (B) holds the right button. Other controller
buttons are consumed in mouse mode. Clicks focus the window under the pointer.
A cursor and “MOUSE MODE” notice show the mode. Exiting mouse mode or losing
the controller releases held mouse buttons. Holding Guide+Start for three seconds
still quits in mouse mode.

Speech phrases map to the same key or text actions:

```toml
[speech]
confirm = { key = "enter" }
"insert greeting" = { text = "Hello" }
```

Phrase matching ignores case, surrounding whitespace and trailing punctuation.
The first initialized plugin subscribed to speech for the focused application replaces these mappings.
Set top-level `unmatched_speech = "type"` before any TOML table to type unmatched
speech into the focused window, followed by one space. Mapped phrases still win.
The transcript keeps its original casing and punctuation. Empty or whitespace-only
transcripts do nothing. The option applies only to the focused window's context.
Without the option, unmatched speech shows “No speech mapping” for three
seconds above the client without changing focus. Record (Share) controls push-to-talk
by default. Configured push-to-talk buttons never reach context mappings.

## Reload

Tehuti watches the context directory and its child directories. Saving any
`context.toml` reloads every context once edits pause for 300 ms; no restart is
needed. A successful reload shows “Contexts reloaded”. If any file fails to read
or parse, Tehuti keeps the contexts it had and shows a notice naming the file and
the start of the error; the full error goes to the log. A reload leaves held
buttons and a speech session in progress untouched. Plugins are not reloaded;
they restart as processes.

## Voice bar

Every output has a 32 logical pixel bar along its bottom edge. Client windows,
including fullscreen windows, stay above it. The compositor draws it without
creating a focusable surface. The left chip names the focused application id,
or Desktop when no window has focus. A listening chip appears while Record is
held; interim text replaces it as transcription previews arrive. Release shows
Transcribing, then the routed action becomes a result chip. Results and errors
clear after three seconds. Focus changes cancel capture and clear voice feedback.

The clock sits at the right edge. Context, live text, results and errors use
different dark backgrounds with chevron edges. Text and bar height follow the
output scale. Existing notices keep their overlay and focus behaviour.

## Keyboard intent

`[keyboard]` sets `on_text_field` to `"button"` (the default), `"open"`, or
`"off"`. `button = "Y"` selects the default opening button; protocol button names
such as `"west"` also work. A context with `app_id = "*"` supplies global keyboard
options. Application contexts inherit each option they omit.

`"button"` opens while a text-input-v3 field is enabled; outside a field the
button follows normal routing. Mouse mode uses the same button. `"open"` opens
once per field. After B closes it, ordinary field updates keep it closed until
the client enables another field. `"off"` disables field opening. Saying
"keyboard" remains available in every application. See `examples/global/context.toml`.

A terminal is an application whose field reports the terminal purpose, or one
whose context sets `terminal = true`. Focusing a terminal never opens the
keyboard, and neither does the opening button. A Guide tap into the app
level opens it unless `open_on_app_level = false`. A terminal keeps the
normal letters and adds a strip row above them: Esc, Tab, Ctrl, Alt, the arrow
keys, Page Up, Page Down, `/`, `-`, `|` and `~`. Strip keys send the draft and
then tap the key. Ctrl and Alt latch for the next key, strip key or letter, so
Ctrl then `c` sends Ctrl-C. The symbols layer carries `|`, `<`, `>` and the
backtick. In a terminal, Start and send type the draft, press Enter and keep the keyboard
open for the next command. B sends Ctrl-C while the shell has
a program in the foreground and closes the keyboard otherwise. Inside tmux,
screen or ssh, B always closes. Dictation into a terminal drops commas and full
stops that end a word.

## Level dimming

At the app level, other visible application windows and their popups dim. The
columns level leaves every window bright. One visible window stays bright.
The powerline, notices, launcher, settings and keyboard stay bright.

The global `*` context supplies defaults; each application's context can override
individual values. Context reload applies these values without restarting.

```toml
[dim]
enabled = true
strength = 0.67
fade_ms = 100
```

`strength` is the retained colour fraction, between 0 and 1, rounded to
thousandths. `fade_ms` is the time to full dimming in milliseconds (0 to 65535).
The defaults shown above apply when the values are absent.

The global context accepts `push_to_talk = ['start']` before `[buttons]` to use Menu for push-to-talk. The default is `['record']`; `[]` disables controller push-to-talk. Listed buttons replace their context bindings; Guide+Start still escapes.

## Column widths

The global context maps "Wider." to `switch-preset-column-width` and
"Narrower." to `switch-preset-column-width-back`. These layout actions cycle
the focused column through the width presets forward and backward, wrapping
at either end.

```toml
[speech]
"Wider." = { layout = "switch-preset-column-width" }
"Narrower." = { layout = "switch-preset-column-width-back" }
```

New columns default to one-third of the available width, the narrowest preset.
`LayoutConfig.default_column_width` and per-window rules can override it.
