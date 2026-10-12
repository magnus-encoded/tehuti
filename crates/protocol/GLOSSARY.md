# Protocol glossary

| Term | Meaning |
| --- | --- |
| Event | Input that reaches the core, such as `button` (name, pressed or released) and `focus` (application id and title). |
| Action | What the core asks its edges to do, such as `key` (a key tap to the focused client). |
| Button name | A controller button named by position: South, East, North, West for the face buttons. South is A on an Xbox pad. |
| Context | A set of input mappings for one application id. |
| Active context | The context selected for the focused window's application id, or none when no context matches. |
| Fallback | The final routing layer, which discards input that earlier layers do not consume. |
| Global layer | The fixed first routing layer, which consumes South presses and sends Enter in every application. |
