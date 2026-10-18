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
Keys are `enter`, `escape`, `space`, `tab`, `backspace`, `up`, `down`, `left`
and `right`. A mapping sends a complete key tap on button press. Releases send
nothing. Unknown fields, buttons and keys are rejected. Application ids must
contain a non-whitespace character.

The global layer consumes South presses and sends Enter. The active context
handles other mapped presses. The fallback discards events that neither handles.
Focus changes select a context by the focused toplevel's current application id.

The escape chord consumes every Guide press and release before context routing.
A context mapping on Guide never fires. Holding Guide for three seconds opens
the system menu once; releasing it before then cancels the hold.

Holding View (Select) for two seconds toggles mouse mode once per hold. A shorter
press sends its normal mapping on release. Left stick moves the pointer with
more speed at larger deflections; right stick scrolls. South (A) holds the left
mouse button for dragging, East (B) holds the right button. Other controller
buttons are consumed in mouse mode. Clicks focus the window under the pointer.
A cursor and “Mouse mode” notice show the mode. Exiting mouse mode or losing
the controller releases held mouse buttons. Holding Guide for three seconds
still opens the system menu in mouse mode.

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
seconds above the client without changing focus. Record (Share) controls
push-to-talk by default. Configured PTT buttons never reach context mappings.

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

The global context (`app_id = '*'`) accepts `push_to_talk = ['record', 'start']`
before `[buttons]`. The default is `['record']`; an empty list disables controller
push-to-talk. Listed buttons give up their normal bindings. While the system menu
is open, presses belong to the menu and releases unwind speech capture.

`keyboard_push_to_talk = 119` sets the keyboard PTT key by Linux evdev code.
The default is Pause (119); Right Ctrl is 97. Right Ctrl participates in the
Ctrl+Alt emergency shortcuts. Meta tapped alone opens the system menu; Meta
held with another key reaches the client as a normal chord.

Elite paddles are `paddle1` (upper right), `paddle2` (lower right), `paddle3`
(upper left), and `paddle4` (lower left). The mappings cover the current grip
codes and the older trigger-happy codes from
[Linux xpad](https://github.com/torvalds/linux/blob/master/drivers/input/joystick/xpad.c).
Unmapped controller key codes appear in debug logs with their values.
