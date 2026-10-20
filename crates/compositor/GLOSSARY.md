# Compositor glossary

| Term | Meaning |
| --- | --- |
| Nested | Running as a window inside the current desktop through Smithay's winit backend. |
| Input source | Anything that yields protocol events for the core step. The evdev pad and the synthetic pad are the two input sources. |
| Evdev pad | A kernel controller, grabbed exclusively so applications only receive what the core forwards. |
| Synthetic pad | An input source driven by code, for tests that run without devices. |
| Apply | Carry out an action from the core step, such as sending a key tap to the focused client. |
| Secret confirmation notice | An overlay naming one secret reference and the focused application before a controller press can authorize lookup. |
| On-screen keyboard | The compositor's own keyboard overlay, docked low. One highlight moves over the keys with the D-pad, the way console keyboards work. |
| Draft | The line of text the on-screen keyboard and speech edit before it is sent to the focused application. Closing the keyboard discards it. |
| Password mode | The on-screen keyboard for a field the application marks as a password: every character except the last one typed is hidden, and push-to-talk never dictates into it. |
| Keyboard intent | A sign from a context that the user wants to type, which opens the on-screen keyboard without a dedicated button. |
| Terminal layout | The on-screen keyboard in a terminal: the normal letters plus a strip of Esc, Tab, latching Ctrl and Alt, arrows and paging keys. B sends Ctrl-C while a program runs in the foreground and closes otherwise. |
| Secret destination | One live Wayland surface during one focus generation; secret insertion checks it again after lookup. |

Starting without a controller logs a warning. Discovery continues every 250 ms and on input directory changes until a controller can be grabbed.
