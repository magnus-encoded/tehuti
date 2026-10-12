# Core glossary

| Term | Meaning |
| --- | --- |
| Step | The pure function `step(state, event) -> (state, actions)`. It performs no I/O; time, plugin replies and effect outcomes arrive as events. The test seam of the core. |
| Layer | One stage of input routing. Every event goes to the global layer, then the active context, then the fallback; the first layer that returns an action consumes the event. |
| Global layer | The fixed mapping that applies in every application. It maps South (A) to an Enter tap. |
| Consumed | An event a layer turned into an action. Later layers never see it. |
| Context | A set of input mappings for one application id. |
| Active context | The context selected for the focused window's application id, or none when no context matches. |
| Fallback | The final routing layer, which discards input that earlier layers do not consume. |
