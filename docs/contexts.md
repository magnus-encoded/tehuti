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
