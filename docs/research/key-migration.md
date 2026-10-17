# Native key mappings

Application contexts map controller buttons and speech phrases to native key
actions. Each context selects an exact `app_id`; the focused application's
context supplies its mappings. No external input injector is required.

Use these values wherever a context accepts an action:

| Shortcut | Native action |
| --- | --- |
| Alt+Left | `{ key = "left", mods = ["alt"] }` |
| Alt+Right | `{ key = "right", mods = ["alt"] }` |
| Ctrl+0 | `{ key = "0", mods = ["ctrl"] }` |
| Ctrl+Equal | `{ key = "equal", mods = ["ctrl"] }` |
| Ctrl+Minus | `{ key = "minus", mods = ["ctrl"] }` |
| Ctrl+F | `{ key = "f", mods = ["ctrl"] }` |
| Ctrl+L | `{ key = "l", mods = ["ctrl"] }` |
| Ctrl+T | `{ key = "t", mods = ["ctrl"] }` |
| Ctrl+W | `{ key = "w", mods = ["ctrl"] }` |
| Ctrl+Tab | `{ key = "tab", mods = ["ctrl"] }` |
| Ctrl+Shift+Tab | `{ key = "tab", mods = ["ctrl", "shift"] }` |
| Shift+Tab | `{ key = "tab", mods = ["shift"] }` |
| Home | `{ key = "home" }` |
| End | `{ key = "end" }` |
| Page Up | `{ key = "page_up" }` |
| Page Down | `{ key = "page_down" }` |
| F5 | `{ key = "f5" }` |
| Ctrl+Alt+Shift+D | `{ key = "d", mods = ["ctrl", "alt", "shift"] }` |

For example, an application can assign New Tab to East and to a speech phrase:

```toml
app_id = "firefox"

[buttons]
east = { key = "t", mods = ["ctrl"] }

[speech]
"new tab" = { key = "t", mods = ["ctrl"] }
```

Keys and modifiers resolve through the active XKB layout, including remapped
modifier keys. An unavailable key or modifier produces no events. Modifiers go
down in Ctrl, Alt, Shift, Super order, followed by the key press and release.
Modifiers then release in reverse order. Duplicate modifier names hold each
modifier once. Omitting `mods` sends an unmodified key.

`tehuti_protocol::KeyName` supplies the shared keyboard table, including
`f1` through `f12`, `home`, `end`, `page_up`, and `page_down`.
All key names serialize in snake_case. The protocol and context configuration
accept the same enum.
