# Plugin protocol v0

A plugin is an executable program. The core starts it with no arguments and
connects its stdin and stdout to pipes. Obey these rules:

- Send one UTF-8 JSON object per line. End each line with a newline.
- Flush stdout after each line.
- Escape newlines inside strings.
- Write logs to stderr.

Each incoming line contains at most 65,536 bytes before the newline.
The output queue of the core holds at most 65,536 bytes, including newlines.
The core logs an event that does not fit. It then declines the event through
the captured fallback.

## Your first plugin

1. Copy [east-to-tab.py](../../examples/plugins/east-to-tab.py) and
   [east-to-tab.json](../../examples/plugins/east-to-tab.json) into
   `<context-directory>/plugins/` (for the session, `~/.config/tehuti/plugins/`).
2. Make the script executable. Change `app_ids` in the manifest to the app id
   of the window you want it for.
3. Start Tehuti with that directory and the app. East now sends Tab there.

To test the plugin without Tehuti, pipe the lines from the
[example session](#example-session) into it. For example, use
`printf '%s\n' '<initialize line>' '<button line>' | ./east-to-tab.py`.

A plugin can answer an event with a plain action list (the example does).
`submit_actions` is an equivalent request form. It carries the event id.
It returns an error object when the core rejects the actions. Start with the
plain list.

A plugin that handles speech lists `speech` in its manifest `events`. It
replies to `speech` requests. Refer to "Events and routing". A context and a
plugin can use different folders and different application ids.

## Validation and errors

Use `"jsonrpc":"2.0"` on every message. Batches are not supported.

The core logs and drops the whole line, with all its actions, in these cases:

- The line has an unknown field at any level.
- The line has an unknown method.
- The line has an invalid value.
- The line has malformed JSON.

The log names the unknown fields. The core parses the next line
independently. Ignore unknown fields that the core sends.

The core rejects an invalid action batch atomically. For `submit_actions`
requests, it sends JSON-RPC error -32602. The error gives the submission id of
the plugin, the index and kind of the rejected action, and the reason. A schema
error names the invalid values or the unknown fields. A permission error names
the missing permission.

For legacy action replies and notices, the core sends an `action_rejected`
notification. Its params contain `event_id` (null for notices) and an `error`
object. The error object has code -32602 and the reason. Notifications never
receive response objects.

The core logs and drops malformed JSON. It also drops lines without a
recognizable action envelope. A plugin can answer any request with a standard
`error` object instead of `result`. For example:

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}
```

The error has an integer `code`, a string `message` and optional `data`.
The core logs the code and message at warn. It treats the reply as a decline
(an empty action list). It completes a matching pending button request. It
never closes the plugin for an error reply. After an initialize error,
initialization stays pending until a valid reply or the existing deadline. If
the core sends an unknown method to a plugin, the plugin can reply with error
-32601 or stay silent. Both mean declined.

The core ignores valid plugin `initialize`, `button`, `speech`, `verb`,
`focus`, `adjuster` and `pad_input` calls. A plugin sends only replies and
these notices: `actions`, `speech.hints`, `speech.language`, `invoke.words`, `verbs`,
`pad_overrides` and `status`. It can also send `submit_actions` requests.

Sometimes an `actions` notice lists a `key` action directly followed by a
`text` action. Then the core waits 50 ms after the key. After that, it applies
the rest of the list in order to the client that has focus at that time. This
lets the client react to the key first.

Ids are unsigned integers from 0 through 18446744073709551615. An id is never
a string, a fraction or a negative number. A reply requires an id. A notice
should have no id (the parser also accepts null on notices). The core assigns
event ids. Plugins assign their own submission ids.

## Initialize

The first message from the core is a request. `core_version` is the version of
the running package. Here it is `0.0.0`:

```json
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocol_version":0,"encoding":"json","core_version":"0.0.0","options":{"action_submission":"submit_actions"}}}
```

`options` lists optional core features. Ignore keys that you do not know.
Reply exactly once:

```json
{"jsonrpc":"2.0","id":0,"result":{"protocol_version":0,"encoding":"json","events":["button","focus"]}}
```

`events` can name any of `button`, `focus` and `speech`. A plugin that handles
only speech replies `["speech"]`. It never receives button requests.

Only version 0 and encoding `json` are supported. The deadline is two seconds
from the creation of the session, before the core queues the request. It is
not counted from the time the plugin receives the request. The core refuses
the session in these cases:

- A reply arrives at or after the deadline.
- A reply has another version.
- A reply has another encoding.

The core logs the refusal. It ignores all later messages from that session.
It sends no refusal reply for an unsupported version or encoding. The session
stays inactive until the process exits or the compositor shuts down. If
initialization times out, the core terminates the process and schedules a
restart.

Before a valid initialize reply, the core ignores all other valid messages.
The core drops malformed replies. A valid retry can succeed before the
deadline. The subscriptions are the intersection of the events in the reply and
the events in the manifest. An empty subscription is allowed. After
initialization, the core ignores duplicate initialize replies.

A focus subscriber receives the latest focus notice if the core observed focus
during initialization. The core does not promise a notice if no focus event
occurred. There is no separate ready notice. The plugin can send actions on the
next line after its initialize reply.

## Events and routing

Button decisions are requests:

```json
{"jsonrpc":"2.0","id":1,"method":"button","params":{"name":"east","state":"pressed"}}
```

Button params carry no application id. A button request concerns the
application that the latest focus notice names. To know which application it
is, subscribe to `focus`. Without a focus subscription, the plugin knows only
that the focused application matches one of the `app_ids` in its manifest.

The states are `pressed` and `released`. The names are `south`, `east`,
`north`, `west`, `left_bumper`, `right_bumper`, `left_trigger`,
`right_trigger`, `select`, `record`, `start`, `guide`, `left_stick`,
`right_stick`, `dpad_up`, `dpad_down`, `dpad_left`, `dpad_right`, `paddle_1`,
`paddle_2`, `paddle_3`, `paddle_4`. All names and enum values are case
sensitive.

These rules apply to the buttons:

- Record (Share) presses and releases control push-to-talk. They never reach
  plugins.
- Guide presses and releases never reach plugins.
- A Guide tap shorter than half a second switches the focus level.
- Holding Guide for half a second opens the system menu.
- Menu buttons stay consumed through release.
- B or Guide closes the menu.
- Holding Guide and Start together for three seconds requests Quit. This also
  applies while the menu is open. If you release either button early, the
  escape chord is cancelled.
- Start does not route while Guide is held.
- Outside overlays, South presses send Enter. If the active context maps
  South, they use that mapping. They never reach plugins. South releases can
  reach plugins.
- Tick is internal. It never crosses the wire.

System menu commands use TOML argument lists in
`$XDG_CONFIG_HOME/tehuti/menu.toml` (or `$HOME/.config/tehuti/menu.toml`).
`TEHUTI_MENU_CONFIG` overrides the path. `examples/menu.toml` shows the format.
Commands run without a shell, with the `WAYLAND_DISPLAY` of the compositor.
Session and power actions require confirmation.

Without overrides, Log Out and Switch to Plasma stop Tehuti with session exit
statuses 21 and 20. Task Monitor launches `btop` in `kitty` when both are
installed. Otherwise, it launches `plasma-systemmonitor`. `task_monitor = [...]`
overrides both. The core detects programs on `PATH` and never installs them.
When it finds none, the notice reads "install btop and kitty, or
plasma-systemmonitor". A launch that fails shows a notice that names the
program.

Reboot and Shut Down have no built-in command, because power commands differ
for each machine. The menu shows them only after `menu.toml` sets them. For
example, use `reboot = ["systemctl", "reboot"]` and
`shut_down = ["systemctl", "poweroff"]`. A command can fail to start, or can
exit with an error within five seconds. Then the core shows a notice that
names the entry and logs one line. `tehuti --check-config` reports malformed
menu commands.

Built-in keys accept argv arrays. An empty array disables a command. Add custom
commands after those keys. Use tables:

```toml
[[entry]]
label = "Terminal"
exec = ["terminal", "--title", "Workspace"]
order = 10
```

Each table requires these values:

- A nonempty label without control characters.
- A nonempty string argv with a nonblank program and no NUL characters.
- An integer order.

Custom entries follow all built-ins. They are sorted by ascending order. Equal
orders keep the file order. Custom commands run on one Confirm press.

The menu config loads at startup. It reloads when the file changes. An invalid
entry produces a notice that names the entry, and the core skips it. Valid
entries remain available. Invalid TOML at startup disables the file and
produces a notice. On a reload, invalid TOML keeps the previous menu and
produces the notice. `--check-config` reports the same errors.

Reply once with the same id and a list of actions:

```json
{"jsonrpc":"2.0","id":1,"result":[{"type":"key","key":"tab"}]}
```

A plugin can also submit its decision as an additive `submit_actions` request.
Initialize advertises this method in `options.action_submission`. Protocol v0
also continues to accept the action-list replies above. The plugin owns the
submission id. `event_id` repeats the event id of the core:

```json
{"jsonrpc":"2.0","id":42,"method":"submit_actions","params":{"event_id":1,"actions":[{"type":"key","key":"tab"}]}}
```

The core replies to submission 42 with `result: []` when it accepts the
decision or the decision is stale. If a pending decision contains an invalid
action, the core replies with `error` code -32602. It then releases the
captured fallback of that event:

```json
{"jsonrpc":"2.0","id":42,"error":{"code":-32602,"message":"action 0 (key): unknown variant `Hyper`"}}
```

Action submissions share the event deadline and the exactly-once handling of
legacy replies. A submission cannot complete the event of a different plugin.
A legacy response object receives no response. Its rejection feedback is a
notification.

An empty list means no action. Button ids start at 1 for each process. Repeat
each id exactly. Replies can arrive out of order. The core ignores late,
duplicate and unmatched replies. There is no cancellation request. Each plugin
can have at most 256 pending requests. The core declines excess requests
immediately.

The button decision budget is 100 milliseconds from dispatch, before queuing.
It is not counted from the time the plugin receives the request. Time and
replies enter the pure core as inputs. The core expires requests at or after
the budget. A missing, invalid or late reply declines the event. Global button
mappings never wait for a plugin. Client delivery has a latency budget of 250
milliseconds while a plugin hangs. Pipe reads and writes are nonblocking and
bounded. This includes the cases when a child stops reading or floods stdout.

Speech decisions are requests to the first matching initialized `speech`
subscriber. Speech requests and button requests route alike. The `app_ids` in
the manifest must contain the focused application id, exactly and case
sensitively. A plugin can subscribe to `speech` without `button`. A verb plugin
has no `app_ids`. The core chooses it by its spoken verb instead. Each request
has a two-second deadline and the same action reply shape:

```json
{"jsonrpc":"2.0","id":2,"method":"speech","params":{"text":"confirm"}}
```

An empty action list declines. The core shows the "No speech mapping" notice
when nothing else claims the speech. A plugin can act on the speech itself. It
then replies `{"jsonrpc":"2.0","id":2,"result":{"handled":true}}`. The request
completes with no actions and no notice. `handled` must be `true`. It is the
only field of the object. Without it, the behaviour does not change.

Speech text is NUL-free. The speech sidecar replies only after release.
Cancelled sessions and replies from failed sidecars never reach plugins.

### Speech hints

An application plugin primes speech recognition with a notice. The notice
replaces the list of the plugin. The plugin can send it at any time. It applies
while a window of the applications of the plugin has focus. A restart clears
it:

```json
{"jsonrpc":"2.0","method":"speech.hints","params":{"hotwords":["chat with guild"]}}
```

Context `[speech] hotwords` and plugin hints share a cap of 64 entries. Context
words come first. If the total is over the cap, the core drops the last entries
of the plugin and logs a warning once.

### Speech language

An application plugin sets the dictation language with a notice. The language
is a lowercase code of 2 to 8 ASCII letters. It is standing: every speech
session that starts while a window of the applications of the plugin has focus
uses it, until the plugin sends another. It is gone when the plugin stops,
crashes or restarts, and while focus is elsewhere. Without it the language is
"en". Interims and the final transcript use the session's language. The core
passes it in the sidecar start command, `{"start":7,"language":"no"}`.

```json
{"jsonrpc":"2.0","method":"speech.language","params":{"language":"no"}}
```

The sidecar reads an optional `speech.toml` in the Tehuti config directory. Its
`[remote]` table maps a language to a transcription socket. A session in that
language goes to the socket, and the local model stays idle. `$XDG_RUNTIME_DIR`
expands in the path:

```toml
[remote]
no = "$XDG_RUNTIME_DIR/tehuti-transcribe.sock"
```

A request is one kind byte (`G` final, `J` interim) and the priming prompt,
a NUL byte, then raw s16 16 kHz mono PCM until the client shuts down writing.
The reply is one JSON line: `{"text":"..."}`, `{"error":"..."}` or
`{"busy":true}`. The whole exchange has one deadline: 30 s for a final, 3 s for an interim. The reply is capped at 64 KiB. Keys must be language codes and paths absolute; the sidecar skips other entries with a warning. On an error, a busy reply or the deadline, a final falls back to the local model with the session's language, and an interim yields nothing.

### Invoke words

An application plugin adds slash-command words to the built-in `invoke` verb
with a notice. The notice replaces the list of the plugin. The plugin can send
it at any time. It applies while a window of the applications of the plugin
has focus. A restart clears it. A crash also clears it:

```json
{"jsonrpc":"2.0","method":"invoke.words","params":{"words":["usage","code-review"]}}
```

Each word is lowercase letters and digits, joined by single hyphens. A notice
holds at most 128 words. Spoken words snap to these words in the same way as
they snap to the `invoke` words of a context. The core tries the words of the
context first.

### Verbs

A verb plugin (`"kind": "verb"`) extends Tehuti itself. It hears only the
speech that begins with one of its own verbs. It hears this speech in any
window, and also with no focused window. It receives nothing else: no buttons,
no focus notices, no application speech.

The core tries speech routes in this order:

1. The own phrases of Tehuti (such as `open ...` and the escape chord).
2. Verbs.
3. Application plugins and context phrases.

The core folds the first word of the transcript with the same sound key as
fuzzy phrase matching. Equal keys match. Keys of five letters or more also
match at a similarity of 0.85. An exception applies when the heard word
extends the verb ("literally" never matches "literal"). The core finds the verb
route once for each utterance. The best score wins. On a tie, the earlier
manifest wins. A verb with no words after it matches nothing.

The core strips the verb, and a following `,` `.` `:` `;` `!` or `?`. The
request carries the verb as the plugin spelled it:

```json
{"jsonrpc":"2.0","id":3,"method":"verb","params":{"verb":"ada","text":"sum up the reviews"}}
```

Replies use the speech deadline and the action shape. An empty reply declines.
The whole transcript then routes as if no verb had matched.

The `verbs` list in the manifest is the starting set. The plugin can replace it
at any time with a notice. The new verbs route only to that plugin. A restart
returns to the manifest list:

```json
{"jsonrpc":"2.0","method":"verbs","params":{"verbs":["ada","orion"]}}
```

A plugin holds at most 16 verbs. Each verb is one word of letters, with at most
32 characters. The core rejects a `verbs` notice from an application plugin in
the same way as an invalid action.

Focus is a notice with no reply:

```json
{"jsonrpc":"2.0","method":"focus","params":{"app_id":"test","title":"Example window"}}
```

`app_id` is a string, or null for no application. `title` is always a string.
It can be empty. It is never null. Focus notices go to every initialized focus
subscriber, even when its manifest does not match the application.

Button requests go to the first matching initialized button subscriber.
Manifests sort by their full filesystem paths, in ascending Rust path order.
`id` does not affect the order. Matching uses the current application id,
exactly and case sensitively. No application matches null. A plugin that is
starting, refused or focus-only does not take part in button selection.

The selected plugin runs before the active context. The core releases the
context actions or fallback actions that it captured at dispatch. It does this
exactly once, in these cases:

- The reply is empty.
- The reply is invalid.
- The reply is missing.
- The reply is late.

Replies cannot complete other requests. Duplicate or stale replies cannot act
on later input. A focus notification settles all pending decisions as
declines. The core releases their captured actions once, in arrival order. It
does this before it updates the active context for later events. Replies for
these settled requests are stale and release nothing. If no plugin qualifies,
normal context routing applies. Global mappings and the escape chord take
priority.

## Actions

Key actions press and release one key in the Wayland client that has focus:

```json
{"type":"key","key":"tab"}
```

Keys are `a`–`z`, `0`–`9`, `minus`, `equal`, `left_bracket`,
`right_bracket`, `semicolon`, `apostrophe`, `grave`, `comma`, `period`,
`slash`, `backslash`, `f1`–`f12`, `home`, `end`, `page_up`, `page_down`,
`insert`, `delete`, `enter`, `escape`, `space`, `tab`, `backspace`,
`up`, `down`, `left`, and `right`. The core also accepts capitalized key labels
such as `Left` and `F5`. Optional `mods` holds `ctrl`, `alt`, `shift`, or
`super`. The core holds them while it taps the key, then releases them:

```json
{"type":"key","key":"l","mods":["ctrl"]}
```

Keys and modifiers resolve through the active XKB keymap, including remapped
modifier keys. A shortcut whose key or modifier keysym is absent produces no
events. The core sends events in this order:

1. The modifiers go down in Ctrl, Alt, Shift, Super order.
2. The key is pressed and released.
3. The modifiers release in reverse order.

Duplicate modifier names hold each modifier once. If you omit `mods`, the core
sends an unmodified key.

Common shortcuts as actions:

| Shortcut | Action |
| --- | --- |
| Alt+Left / Alt+Right | `{ key = "left", mods = ["alt"] }` / `"right"` |
| Ctrl+0, Ctrl+Equal, Ctrl+Minus | `{ key = "0", mods = ["ctrl"] }`, `"equal"`, `"minus"` |
| Ctrl+F, Ctrl+L, Ctrl+T, Ctrl+W | `{ key = "f", mods = ["ctrl"] }`, `"l"`, `"t"`, `"w"` |
| Ctrl+Tab | `{ key = "tab", mods = ["ctrl"] }` |
| Ctrl+Shift+Tab | `{ key = "tab", mods = ["ctrl", "shift"] }` |
| Shift+Tab | `{ key = "tab", mods = ["shift"] }` |
| Home, End, Page Up, Page Down, F5 | `{ key = "home" }`, `"end"`, `"page_up"`, `"page_down"`, `"f5"` |
| Ctrl+Alt+Shift+D | `{ key = "d", mods = ["ctrl", "alt", "shift"] }` |

Context mappings use the same fields. For example, use
`{ key = "left", mods = ["alt"] }`. The shipped Firefox context maps East to
Back, North to the address bar, and the bumpers to previous tab and next tab.
Its speech commands are "new tab", "reload", and "close tab".

A button mapping with `hold = true` keeps its key down while the button is
held. Use it for programs, such as emulators, that read the keyboard once for
each frame and miss a tap:

```toml
[buttons]
south = { key = "x", hold = true }
```

The key goes down on the press and up on the release. These events release the
key early:

- A focus change.
- A controller disconnect.
- The system menu.
- The settings view.
- A passthrough context.

If two buttons hold one key, the core presses the key once. It releases the key
with the last release. A held key takes no `mods`. It cannot be a phrase
mapping. Plugins cannot hold keys: the `key_down` and `key_up` actions are
reserved for the core.

Text delivers Unicode through enabled text-input-v3. Keyboard taps are the
fallback:

```json
{"type":"text","text":"Hello ✓"}
```

Text can be empty but must not contain NUL. There is no separate limit for the
length of text. The whole JSON line must fit the line limit. Count UTF-8 bytes
and JSON escapes. Text-input commits split at character boundaries into
commits of at most 4000 bytes. The keyboard fallback skips characters that are
absent from the keymap and logs them. A newline becomes Enter. Without a
focused client, key and text actions have no effect.

X11 clients run through `xwayland-satellite`. It receives the same configured
keymap as Wayland clients (tested). Key taps therefore reach X11 clients as
ordinary key events. Text-input-v3 is a Wayland protocol. An X11 client may not
enable it. Text then arrives through the keyboard fallback.

Contexts can launch an installed desktop entry, close the focused window, or
show an overlay notice. Both pressed-button mappings and exact phrase mappings
accept these TOML values:

```toml
app_id = "example"
[buttons]
east = { launch = "org.example.App.desktop" }
west = { close = "focused" }
north = { notify = "Ready" }
[speech]
"open app" = { launch = "org.example.App.desktop" }
"close app" = { close = "focused" }
"show notice" = { notify = "Ready" }
```

Their action shapes are:

```json
{"type":"launch","desktop_entry":"org.example.App.desktop"}
{"type":"close"}
{"type":"notice","text":"Ready"}
```

These actions are reserved for core contexts. Plugins cannot submit them.
Context mappings also reach four more reserved actions. Their JSON shapes are
`{"type":"restart"}`, `{"type":"layout","action":"focus-column-left"}`,
`{"type":"toggle_ticker"}` and `{"type":"undo_dictation"}`. The core also uses
`{"type":"key_down","key":"x"}`, `{"type":"key_up","key":"x"}` and
`{"type":"secret_request","reference":{"entry":"mail"}}` itself. If a plugin
sends any of them, the core rejects the whole batch as "reserved for the
core". For the TOML forms, refer to "Mapping values" in the
[context guide](../contexts.md).

Launch uses `gtk-launch`, which must be installed. It uses the Wayland socket
of the compositor. Desktop entry ids include `.desktop`. They match entries
below `$XDG_DATA_HOME/applications` (default `~/.local/share/applications`)
and `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`). Subdirectory
separators become hyphens in the id. Discovery skips directory symlinks. It
stops at sixteen directory levels. The desktop entry cache scans once at
startup. It refreshes every thirty seconds on a worker thread. Launch reads the
cached ids.

These cases show `APPLICATION UNAVAILABLE` without a change of focus:

- The id is unknown.
- The launcher is unavailable.
- The launch fails.

Launch itself does not select a window. A newly mapped application window
receives focus. At most sixteen launcher processes can be pending. Excess
launches show the same notice. The compositor polls launch completion without
blocking input.

Close sends the focused client a close request. It removes the window of the
client from the mapped set. It then gives focus to the most recently mapped
remaining window. The client can finish its shutdown later. Close does nothing
when no window has focus.

The `notify` TOML key produces a Notice action. The action displays NUL-free
text for three seconds without taking keyboard focus. The overlay renders two
lines of forty characters. It uses uppercase Latin letters, digits and
punctuation. Unsupported characters appear as spaces. A later notice replaces
the previous one. Secret confirmation overlays take priority over ordinary
notices.

Quit requests controller release and compositor shutdown:

```json
{"type":"quit"}
```

Actions run in list order. Permitted actions before Quit run. After a
permitted Quit, the core ignores further plugin actions, including later lines
and plugins. Shutdown happens when the event loop next checks the Quit request.

A plugin can send actions without a request:

```json
{"jsonrpc":"2.0","method":"actions","params":{"actions":[{"type":"key","key":"space"}]}}
```

These notices need no event subscription or application match. They require
completed initialization. They have no id, no response and no button deadline.
They are independent actions. They never complete a pending request. `notice`
actions are reserved for the core, like `launch` and `close`.

Every action requires its matching manifest permission: `key`, `text`, or
`quit`. This applies to actions in replies, in `submit_actions` and in notices.
For example, a reply with a `text` action needs the `text` permission. The core
rejects the entire batch if any action lacks permission or is a core-only
action. The core delivers no action of a rejected batch. Permissions restrict
protocol actions. They do not sandbox the process. They do not restrict its OS
access.

Unsolicited actions target the focus at the time the core applies them. A focus
change settles pending request decisions as declines. It releases their
fallback actions once.

Secret references are objects shaped as `{"entry":"mail"}`. A core context can
bind one to a controller button. The compositor asks for fresh confirmation
before lookup and delivery. The core rejects requests that come from plugins.
There is no plugin permission for secret access. Never put secret contents in
text, events, actions, plugin messages or logs.

## Manifest and installation

Place manifests directly in `<context-directory>/plugins/`. Start the
compositor as `tehuti <context-directory> <application> [arguments...]`.
Refer to the [manifest schema](manifest.schema.json). Required fields:

```json
{"executable":"./example.py","app_ids":["test"],"events":["button","focus"],"start":"always","permissions":["key"]}
```

`id`, `author` and `security_contact` are optional strings or null.
The core rejects unknown fields. Manifests have the same 65,536-byte limit.
Executable and application ids must not be blank. The core does not trim
whitespace. Empty lists and duplicate entries are allowed. Events are
`button`, `focus` and `speech`. Permissions are `key`, `text` and `quit`.

`kind` is `application` (the default) or `verb`. It sets which permissions a
plugin can request. An application plugin serves its `app_ids` while one of
them has focus. It declares no `verbs`. A verb plugin follows these rules:

- It lists its starting `verbs`.
- It has empty `app_ids`.
- It has no adjusters.
- It has only the `speech` event.
- It has only the `text` permission.
- It has `start` set to `always`.

```json
{"kind":"verb","verbs":["ada"],"executable":"./chief.py","app_ids":[],"events":["speech"],"start":"always","permissions":["text"]}
```

A plugin that needs both behaviours ships as two plugins.

The core reads only direct entries with the case-sensitive extension `.json`.
There is no recursive search and no glob expansion. The core rejects manifest
symlinks and nonregular files. It logs bad manifests and skips each one
separately.

An executable path can be absolute or relative to the plugin directory,
including `..`. The path is not confined to that directory. The core resolves a
relative context directory from the working directory of the compositor. The
child inherits that working directory and the environment, including the
`WAYLAND_DISPLAY` of the desktop. The core does not give it the socket of the
nested application. No shell expands the path. Make the program executable and
use a shebang. Install its interpreter yourself.

`start` is `always` (launch during setup) or `on_focus` (launch on the first
matching focus, then keep running after focus changes). Empty application-id
lists never match. An `on_focus` plugin with such a list never starts. After a
plugin starts, the supervisor keeps it under supervision after focus changes.
These events schedule a restart:

- A failed launch.
- A process exit.
- Stdout EOF.
- A pipe failure.

There is no shutdown message or acknowledgement. On stdout EOF or a pipe
failure, the core disables the session. It discards an unfinished final line.
On compositor teardown, the core kills and reaps child processes. A plugin
should also exit if stdin reaches EOF. Do not rely on a graceful shutdown
interval.

## Supervision

The supervisor checks processes every 10 milliseconds, even without input.
Restart backoff starts at 100 milliseconds. It doubles after each failure and
caps at 30 seconds. It resets after 30 seconds of initialized operation without
invalid messages. Initialization alone does not reset the backoff. Each failure
logs the executable, the reason and the restart delay.

The core terminates a plugin that has not initialized within two seconds. After
initialization, the core restarts a plugin with backoff if it misses three
decisions in a row. A reply before its decision deadline resets the count of
consecutive misses. The shorter decision deadlines of the core still release
captured fallback actions on time. Idle plugins with no outstanding decisions
can stay silent. Request ids continue across restarts. A new process therefore
cannot complete a pending decision of an earlier process.

The core drops an isolated malformed or overlong line independently. Sixteen
invalid lines or rejected action batches, without 30 seconds of healthy
initialized operation, restart the plugin with backoff. Valid error replies are
declines. They do not count as invalid messages.

Each stdout callback reads at most 16 chunks of 4096 bytes. Each process can
send at most 262,144 bytes in a one-second window. The window begins at launch,
or at the first read after the preceding window expires. If a plugin exceeds
this limit, the core terminates it and schedules a restart. Line storage and
the stdin queue each stay bounded at 65,536 bytes. Outstanding decisions stay
bounded at 256. Reads, writes and process status checks do not block. The core
reaps terminated children outside the compositor loop. The pure core step
performs no plugin I/O.

## Example session

This complete session uses the manifest above with both event subscriptions.
Each JSON row is one line on the named pipe. The labels are not wire data.
Focus changes to `test` while initialization is pending.

| Direction | Line |
| --- | --- |
| Core → stdin | `{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocol_version":0,"encoding":"json","core_version":"0.0.0","options":{"action_submission":"submit_actions"}}}` |
| Plugin → stdout | `{"jsonrpc":"2.0","id":0,"result":{"protocol_version":0,"encoding":"json","events":["button","focus"]}}` |
| Core → stdin | `{"jsonrpc":"2.0","method":"focus","params":{"app_id":"test","title":"Example window"}}` |
| Core → stdin | `{"jsonrpc":"2.0","id":1,"method":"button","params":{"name":"east","state":"pressed"}}` |
| Plugin → stdout | `{"jsonrpc":"2.0","id":1,"result":[{"type":"key","key":"tab"}]}` |
| Core → stdin | `{"jsonrpc":"2.0","id":2,"method":"button","params":{"name":"east","state":"released"}}` |
| Plugin → stdout | `{"jsonrpc":"2.0","id":2,"result":[]}` |
| Plugin → stdout | `{"jsonrpc":"2.0","method":"actions","params":{"actions":[{"type":"key","key":"space"}]}}` |
| Core → stdin | `{"jsonrpc":"2.0","method":"focus","params":{"app_id":null,"title":""}}` |

The user holds Guide and Start for three seconds. The core sends no chord
message and no Tick message. It requests Quit and terminates the plugin during
teardown. For a runnable button remapping plugin, refer to
[east-to-tab.py](../../examples/plugins/east-to-tab.py). You do not need a
Tehuti library. Use the JSON library of your language and line input and
output.

## Controller adjusters

A manifest can register up to 32 named adjusters ahead of time:

```json
"adjusters": [{"name":"contrast","minimum":0,"maximum":200,"value":80}]
```

Names normalize like speech phrases. Each name must be unique within the
manifest and must contain at most 24 characters after normalization. `volume`
is reserved. Ranges use signed 32-bit integers, with `minimum < maximum`.
`value` is inside the range. The core rejects a manifest with unknown
fields or invalid ranges.

The user says a registered name. The gauge for that name opens when exactly
one initialized plugin for the focused application supplies the name. The
vertical axis of the left stick adjusts the value, at up to half the range for
each second. These actions commit the value: returning the stick to rest, or
pressing South. East reverts to the opening value. These events also revert
the value: focus loss, a controller disconnect, or a higher overlay. Buttons
stay consumed through release. The core runs the loop without requesting
plugin decisions. The registering plugin receives notices for previews and for
completion:

```json
{"jsonrpc":"2.0","method":"adjuster","params":{"name":"contrast","value":90,"finished":false}}
{"jsonrpc":"2.0","method":"adjuster","params":{"name":"contrast","value":80,"finished":true}}
```

A completion carries the committed value or the reverted value. The next
opening starts at that value. Plugins that register no adjusters receive no
adjuster notices.

Saying `volume` opens the built-in 0–100 percent adjuster. The compositor uses
`wpctl` on a worker thread to read and preview the volume of the default audio
sink. If a command fails, the core shows `VOLUME UNAVAILABLE`. Input routing
keeps running. The gauge appears above the bar. Speech inside the keyboard
remains dictation.

## Virtual pad overrides

A context opts in with `[pad] passthrough = true` and `virtual = true`. While
its passthrough is active, Tehuti keeps the physical controller. It presents a
virtual controller to the application. The core copies physical inputs to the
virtual controller as they arrive, unless they match an override.

A plugin sets its overrides ahead of time with a notice. Each call replaces the
whole set of that plugin. An empty list clears it:

```json
{"jsonrpc":"2.0","method":"pad_overrides","params":{"overrides":[{"button":"south","held":["left_bumper"]}]}}
```

An override matches a press of `button` while every button in `held` is
already held on the physical pad. `held` is optional and defaults to empty.
Button names are those of button events, except `guide` and `record`. The core
rejects these two. A plugin can override South like any other button. Its
usual Enter mapping does not apply to a consumed press.

The core accepts at most 64 overrides for each call. It drops an invalid call
whole, like any invalid line, and keeps the previous set. Overrides need
completed initialization. They need no event subscription and no permission.
The core holds them for each plugin. It clears them when the plugin exits.

The core applies overrides itself. It does not ask the plugin for each press.
Only plugins that match the focused application and are initialized take part.
The first plugin in manifest order whose override matches receives the input.
The core does not copy a matched press and its release to the virtual
controller. It sends them to that plugin as notices, with no reply:

```json
{"jsonrpc":"2.0","method":"pad_input","params":{"button":"south","state":"pressed"}}
{"jsonrpc":"2.0","method":"pad_input","params":{"button":"south","state":"released"}}
```

A release follows its press. A consumed press always has a consumed release.
A copied press always has a copied release. This is true even if the overrides
change in between. The core copies held buttons in a condition normally. Inputs
that match no override keep their order and timing. This includes sticks,
triggers and the D-pad. If a plugin wants to act on a pad input, it answers
with an `actions` notice. Contexts without `virtual = true` read the physical
pad as before. `pad_overrides` has no effect on them.

## Powerline status

A plugin can show one short status text on the powerline bar, for example the
current chat channel. It is a notice with no reply. It needs no permission:

```json
{"jsonrpc":"2.0","method":"status","params":{"text":"Guild"}}
{"jsonrpc":"2.0","method":"status","params":{"text":null}}
```

Each notice replaces the previous status of the plugin. `null` clears it. Text
holds at most 32 characters and no control characters. The core drops a line
with longer or invalid text, like any invalid line. The status shows only when
both of these conditions are true:

- The focused application matches the `app_ids` in the manifest of the plugin.
- The plugin is initialized.

When several plugins qualify, the first in manifest order shows. A plugin exit
clears its status.
