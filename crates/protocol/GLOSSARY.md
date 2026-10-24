# Protocol glossary

| Term | Meaning |
| --- | --- |
| Event | Input that reaches the core, such as `button` (name, pressed or released) and `focus` (application id and title). |
| Action | What the core asks its edges to do, such as `key` (a key tap to the focused client). |
| Button name | A controller button named by position: South, East, North, West for the face buttons. South is A on an Xbox pad. |
| Context | A set of input mappings for one application id. |
| Active context | The context selected for the focused window's application id, or none when no context matches. |
| Fallback | The final routing layer, which discards input that earlier layers do not consume. |
| Global layer | The fixed first routing layer, which consumes South presses and sends Enter in every application, unless the active context maps South. |
| Held key | A key a `hold = true` button mapping keeps down until the button's release, sent as the core-only `key_down` and `key_up` actions. |
| Tick | Internal clock data carrying elapsed time into the core. It must never cross the plugin wire. |
| Quit | The action that asks the compositor to release input and stop its event loop. |
| Escape chord | Holding Guide for three seconds to emit one Quit. It consumes all Guide presses and releases before routing; an early release cancels the hold. |
| Speech | Validated transcription text routed to the focused context or subscribed plugin. |
| Record | The Share button, KEY_RECORD, reserved for push-to-talk. |
| Notice | A core action displaying unmatched speech feedback without changing focus. |
| Unmatched speech | A transcript with no phrase mapping. A context may opt into typing it with a trailing space through `unmatched_speech = "type"`. |
