---
name: write-a-context
description: Write or tune a Tehuti context.toml for one application. Use when the owner wants pad controls, voice commands or ticker hints for an app, or says an app's context feels wrong.
---

# Write a context

A context (`~/.config/tehuti/<app>/context.toml`) decides what the pad, voice
and ticker do while one app has focus. It hot-reloads, so the work is a loop:
edit, the owner tries it, repeat.

**Voice names a destination; the pad takes a step on the path.** A phrase earns
its place when the owner can say what they want and the app can jump straight
there: a named view, a menu, a mode, a typed code. Steps, cycles, increments,
aiming and anything held stay on the pad, which walks the owner there.

A context file only sends input one way. When the context needs **two-way**
communication (reading the app's state, answering it, or changing behaviour
based on what the app reports), write a plugin instead, over the JSON-RPC
protocol in
[docs/protocol/README.md](https://github.com/magnus-encoded/tehuti/blob/main/docs/protocol/README.md).

## Steps

1. **Identify the window.** Read the focused `app_id` (and `title` if the id is
   shared) from `~/.local/state/tehuti/session.log`. Done when you have the
   exact match.
2. **Read the schema** for the installed version (`tehuti --version`):
   [`docs/contexts.md`](https://github.com/magnus-encoded/tehuti/blob/main/docs/contexts.md) and
   [`crates/protocol/src/lib.rs`](https://github.com/magnus-encoded/tehuti/blob/main/crates/protocol/src/lib.rs)
   on GitHub, read at the commit `tehuti --version` prints rather than
   `main`. Done when every key and field you plan to
   use exists there.
3. **Collect the app's controls**, most trusted first: its own bindings screen
   (ask the owner for a screenshot), its manual (render scanned PDFs to images),
   then a web search. Done when every control is confirmed or marked
   unverified.
4. **Decide the pad.** If the app reads gamepads itself, pass the pad through
   and let it own analog input; otherwise map buttons to keys, with `hold` for
   held actions. Write the app's button map in the header comment.
5. **Choose the destinations.** One phrase per destination, in words that are
   easy to say, and distinct from the global `*` phrases. Codes typed into a
   field become `text` phrases. Done when each phrase passes the destination
   test and none collides.
6. **Set the ticker** to `speech = "icons"` and give icons only to the phrases
   the owner will use most. Games open fullscreen.
7. **Write and confirm.** Back up an existing file, write the new one, and
   run `tehuti --check-config`, then check the log for a fresh
   `contexts reloaded`.
8. **Iterate** from the owner's screenshots, one change per round. When the
   fix belongs in Tehuti rather than the config, file an issue with log lines
   and `file:line` evidence, and give the owner the workaround.

Done when the context reloads cleanly, every key matches a confirmed control,
the owner has used it, and every Tehuti gap is filed as an issue.
