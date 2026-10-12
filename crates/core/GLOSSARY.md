# Core glossary

| Term | Meaning |
| --- | --- |
| Step | The pure function `step(state, event) -> (state, actions)`. It performs no I/O; time, plugin replies and effect outcomes arrive as events. The test seam of the core. |
| Layer | One stage of input routing. The escape chord runs first, then the global layer, the active context and the fallback; consumption stops routing. |
| Global layer | The fixed mapping that applies in every application. It maps South (A) to an Enter tap. |
| Consumed | An event handled by a layer, with or without an action. Later layers never see it. |
| Context | A set of input mappings for one application id. |
| Active context | The context selected for the focused window's application id, or none when no context matches. |
| Fallback | The final routing layer, which discards input that earlier layers do not consume. |
| Tick | Internal clock data carrying elapsed time into the core. It must never cross the plugin wire. |
| Quit | The action that asks the compositor to release input and stop its event loop. |
| Escape chord | Holding Guide for three seconds to emit one Quit. It consumes all Guide presses and releases before routing; an early release cancels the hold. |
| Capture session | A core-owned speech start paired with exactly one stop, independently of sidecar I/O. |
| Secret destination | One focused surface during one focus generation; returning to the same surface creates a different destination. |
| Secret confirmation | A one-use controller approval bound to a named reference and secret destination, with a short expiry. |
