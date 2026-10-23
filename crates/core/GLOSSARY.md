# Core glossary

| Term | Meaning |
| --- | --- |
| Step | The pure function `step(state, event) -> (state, actions)`. It performs no I/O; time, plugin replies and effect outcomes arrive as events. The test seam of the core. |
| Dispatch | The pure function `dispatch::step(state, input) -> (state, effects)` that fixes the order of routing work for one tick, button, speech or focus input. It calls the step, the plugin step and the speech step. |
| Effect | Work dispatch asks the compositor to carry out. A query effect waits for its reply input before dispatch continues. |
| Layer | One stage of input routing. The escape chord runs first, then the columns level for LB and RB, the global layer, held keys, the active context and the fallback; consumption stops routing. |
| Global layer | The fixed mapping that applies in every application. It maps South (A) to an Enter tap by default; an active context that maps South replaces it. |
| Held key | A key a `hold = true` button mapping keeps down from the button's press to its release, or until focus, a disconnect or an overlay releases it. |
| Consumed | An event handled by a layer, with or without an action. Later layers never see it. |
| Context | A set of input mappings for one application id. |
| Active context | The context selected for the focused window's application id, or none when no context matches. |
| Global context | The context with application id `*`. Its mapped buttons and phrases apply wherever the active context maps none; a phrase it maps beats the active context's `unmatched_speech` typing. |
| Fallback | The final routing layer, which discards input that earlier layers do not consume. |
| Tick | Internal clock data carrying elapsed time into the core. It must never cross the plugin wire. |
| Quit | The action that asks the compositor to release input and stop its event loop. |
| Escape chord | Holding Guide and Start together for three seconds to emit one Quit. It consumes all Guide presses and releases before routing; an early release cancels the hold. |
| Level | What LB and RB drive. The app level (the start state) routes them to the focused app like other buttons; columns moves focus one column left or right. A Guide tap shorter than half a second, with Start untouched, toggles the level; the bar lists open window titles at columns level and the focused context at app level. Focus changes keep it. |
| Capture session | A core-owned speech start paired with exactly one stop, independently of sidecar I/O. |
| Secret destination | One focused surface during one focus generation; returning to the same surface creates a different destination. |
| Secret confirmation | A one-use controller approval bound to a named reference and secret destination, with a short expiry. |

The system menu opens after holding Guide for half a second. A shorter tap
keeps switching the focus level. The menu consumes controller input before the
keyboard, mouse mode, contexts and plugins. B or Guide closes it. Session and
power commands require confirmation with a second A press.
