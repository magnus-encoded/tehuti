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
A context mapping on Guide never fires. Holding Guide for three seconds emits
Quit once; releasing it before then cancels the hold.

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
seconds above the client without changing focus. Record (Share) controls push-to-talk and
never reaches context mappings.
