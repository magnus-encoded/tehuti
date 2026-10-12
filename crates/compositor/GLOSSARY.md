# Compositor glossary

| Term | Meaning |
| --- | --- |
| Nested | Running as a window inside the current desktop through Smithay's winit backend. |
| Input source | Anything that yields protocol events for the core step. The evdev pad and the synthetic pad are the two input sources. |
| Evdev pad | A kernel controller, grabbed exclusively so applications only receive what the core forwards. |
| Synthetic pad | An input source driven by code, for tests that run without devices. |
| Apply | Carry out an action from the core step, such as sending a key tap to the focused client. |
| Secret confirmation notice | An overlay naming one secret reference and the focused application before a controller press can authorize lookup. |
| Secret destination | One live Wayland surface during one focus generation; secret insertion checks it again after lookup. |
