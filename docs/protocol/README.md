# Plugin protocol v0

A plugin is an executable program. The core starts it with no arguments and
pipes stdin and stdout. Send one UTF-8 JSON object per line, ending with a
newline, and flush stdout. Escape newlines inside strings. Log to stderr.
Each incoming line may contain at most 65,536 bytes before the newline.
The core's output queue holds at most 65,536 bytes, including newlines;
events that do not fit are logged and declined through the captured fallback.

## Validation and errors

Use `"jsonrpc":"2.0"` on every message. Batches are not supported.
Unknown fields at any level, unknown methods, invalid values and malformed
JSON cause the whole line to be logged and dropped, including all its actions.
Unknown fields are named in the log. The next line is parsed independently.
Ignore unknown fields sent by the core.

The core rejects an invalid action batch atomically and sends JSON-RPC error
-32602 for `submit_actions` requests, using the plugin's submission id, the
rejected action index and kind, and its reason. Schema errors name invalid
values or unknown fields; permission errors name the missing permission.
Legacy action replies and notices receive an `action_rejected` notification
whose params contain `event_id` (null for notices) and an `error` object with
code -32602 and the reason. Notifications never receive response objects.
Malformed JSON and lines without recognizable action envelopes are logged and
dropped. A plugin may answer any request with a standard `error` object instead
of `result`, for example:

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}
```

The error has an integer `code`, a string `message` and optional `data`.
The core logs the code and message at warn and treats the reply as declining
(an empty action list), completing a matching pending button request. It never
closes the plugin for an error reply. An initialize error leaves initialization
pending until a valid reply or the existing deadline. For unknown methods sent
to a plugin, it may reply with error -32601 or stay silent; both mean declined.
Valid plugin `initialize`, `button` and `focus` calls are ignored: plugins send
only replies and `actions` notices.

Ids are unsigned integers from 0 through 18446744073709551615, never strings,
fractions or negative numbers. Replies require an id; notices should omit it
(the parser also accepts null on notices). The core assigns event ids; plugins
assign their own submission ids.

## Initialize

The first core message is a request. `core_version` is the running package's
version, shown here as `0.0.0`:

```json
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocol_version":0,"encoding":"json","core_version":"0.0.0"}}
```

An optional `options` object is reserved for future needs. Reply exactly once:

```json
{"jsonrpc":"2.0","id":0,"result":{"protocol_version":0,"encoding":"json","events":["button","focus"]}}
```

Only version 0 and encoding `json` are supported. The deadline is two seconds
from session creation, before the request is queued, not from receipt by the
plugin. A reply received at or after the deadline, or with another version or
encoding, refuses the session. Refusal is logged; all later messages from that
session are ignored. An unsupported version or encoding receives no refusal reply and stays
inactive until the process exits or the compositor shuts down. An initialization
timeout terminates the process and schedules a restart.

Before a valid initialize reply, all other valid messages are ignored.
Malformed replies are dropped; a valid retry can succeed before the deadline.
Subscriptions are the intersection of the reply's events and the manifest's
events. Empty subscriptions are allowed. After initialization, duplicate
initialize replies are ignored. A focus subscriber receives the latest focus
notice if the core has observed focus during initialization. No notice is
promised if no focus event has occurred. There is no separate ready notice:
the plugin may send actions on the next line after its initialize reply.

## Events and routing

Button decisions are requests:

```json
{"jsonrpc":"2.0","id":1,"method":"button","params":{"name":"east","state":"pressed"}}
```

States are `pressed` and `released`. Names are `south`, `east`, `north`,
`west`, `left_bumper`, `right_bumper`, `select`, `record`, `start`, `guide`, `left_stick`,
`right_stick`, `dpad_up`, `dpad_down`, `dpad_left`, `dpad_right`.
All names and enum values are case sensitive.

Record (Share) presses and releases control push-to-talk and never reach plugins.
Guide presses and releases never reach plugins. Holding Guide for three
seconds opens the core's system menu; releasing early cancels the hold. While
the menu is open it takes every button and speech input, and no plugin sees
any. The d-pad moves, South selects, East or Guide closes. Other buttons still
route while Guide is held. South presses send Enter and never reach plugins;
South releases can reach them. Tick is internal and never crosses the wire.

Reply once with the same id and a list of actions:

```json
{"jsonrpc":"2.0","id":1,"result":[{"type":"key","key":"tab"}]}
```

Plugins may submit their decision as an additive `submit_actions` request.
Initialize advertises this method in `options.action_submission`; protocol v0
also continues to accept the action-list replies above. The plugin owns the
submission id; `event_id` echoes the core's event id:

```json
{"jsonrpc":"2.0","id":42,"method":"submit_actions","params":{"event_id":1,"actions":[{"type":"key","key":"tab"}]}}
```

The core replies to submission 42 with `result: []` when the decision is
accepted or stale. If a pending decision contains an invalid action, it replies
with `error` code -32602 and releases that event's captured fallback:

```json
{"jsonrpc":"2.0","id":42,"error":{"code":-32602,"message":"action 0 (key): unknown variant `Tab`"}}
```

Action submissions share the event deadline and exactly-once handling of legacy
replies. A submission cannot complete a different plugin's event. There is no
response to a legacy response object; its rejection feedback is a notification.

An empty list means no action. Button ids start at 1 per process. Echo each id exactly.
Replies may arrive out of order. Late, duplicate and unmatched replies are
ignored. There is no cancellation request. At most 256 requests may be pending
per plugin. Excess requests decline immediately. The button decision budget is
100 milliseconds from dispatch, before queuing, not from the plugin's receipt.
Time and replies enter the pure core as inputs; the core expires requests at
or after the budget. A missing, invalid or late reply declines the event.
Global button mappings never wait for a plugin; client delivery while a plugin
hangs has a 250-millisecond latency budget. Pipe reads and writes are nonblocking
and bounded, including when a child stops reading or floods stdout.

Speech decisions are requests to the first matching initialized `speech`
subscriber, with a two-second deadline and the same action reply shape:

```json
{"jsonrpc":"2.0","id":2,"method":"speech","params":{"text":"confirm"}}
```

Speech text is NUL-free. The speech sidecar replies only after release.
Cancelled sessions and replies from failed sidecars never reach plugins.

Focus is a notice, with no reply:

```json
{"jsonrpc":"2.0","method":"focus","params":{"app_id":"test","title":"Example window"}}
```

`app_id` is a string or null for no application. `title` is always a string,
possibly empty, never null. Focus notices go to every initialized focus
subscriber, even when its manifest does not match the application.

Button requests go to the first matching, initialized button subscriber.
Manifests sort by their full filesystem paths, in ascending Rust path order;
`id` does not affect order. Matching uses the current application id, exactly
and case sensitively. No application matches null. A starting, refused or
focus-only plugin does not take part in button selection.

The selected plugin runs before the active context. An empty, invalid, missing
or late reply releases the context or fallback actions captured when the
request was dispatched, exactly once. Replies cannot complete other requests,
and duplicate or stale replies cannot act on later input. A focus notification
settles all pending decisions as declines, releasing their captured actions once
in arrival order before updating the active context for later events. Replies
for these settled requests are stale and release nothing. If no plugin qualifies,
normal context routing applies. Global mappings and the escape chord take priority.

## Actions

Key actions press and release one key in the currently focused Wayland client:

```json
{"type":"key","key":"tab"}
```

Keys are `enter`, `escape`, `space`, `tab`, `backspace`, `up`, `down`, `left`,
`right`, in lowercase. Text delivers Unicode through enabled text-input-v3,
with keyboard taps as fallback:

```json
{"type":"text","text":"Hello ✓"}
```

Text may be empty but must not contain NUL. There is no separate text-length
limit: the whole JSON line must fit the line limit, counting UTF-8 bytes and
JSON escapes. Text-input commits split at character boundaries into at most
4000 bytes. Keyboard fallback skips characters absent from the keymap and
logs them; a newline becomes Enter. Without a focused client, key and text
actions have no effect.

The compositor keyboard opens when the focused text-input-v3 client commits
`enable`, or through a context mapping:

```toml
[buttons.left_stick]
keyboard = "open"
```

`{"type":"keyboard"}` is reserved for core contexts. Each stick selects a key
on its QWERTY half; its bumper types it. Neutral keeps the selection. North
cycles letters, capitals and symbols; West sends Backspace, East closes, and
South sends Enter. Guide and Record retain their reserved actions. A committed
`disable`, focus loss or text-input destruction closes the keyboard. Ordinary
text-input commits preserve an East dismissal; an explicit enable reopens it.

The keyboard uses the same text-input-v3 delivery and keymap fallback as `text`.
Its characters and key taps never enter the log, including unavailable fallback
characters. Vector keys and font outlines rasterize with coverage at the output
mode's resolution. The overlay sits above the powerline bar and moves to the top
when the client's committed cursor rectangle overlaps it. Completion providers
in #114 can consume the keyboard's text effects at commit rate; completion is
not implemented.

Contexts can launch an installed desktop entry, close the focused window, or
show an overlay notice. Both pressed-button and exact phrase mappings accept
these TOML values:

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

These actions are reserved for core contexts; plugins cannot submit them.
Launch uses `gtk-launch`, which must be installed, with the compositor's Wayland
socket. Desktop entry ids include `.desktop` and match entries beneath
`$XDG_DATA_HOME/applications` (default `~/.local/share/applications`) and
`$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`). Subdirectory separators
become hyphens in the id. Directory symlinks are skipped and discovery stops
at sixteen directory levels. The desktop entry cache scans once at startup and
refreshes every thirty seconds on a worker thread. Launch reads the cached ids.
Unknown ids, unavailable launchers and failed
launches show `APPLICATION UNAVAILABLE` without changing focus. Launch itself
does not select a window; a newly mapped application window receives focus.
At most sixteen launcher processes may be pending; excess launches show the
same notice. The compositor polls launch completion without blocking input.

Close sends the focused client a close request, removes its window from the
mapped set and focuses the most recently mapped remaining window. The client
may finish shutting down later. Closing without a focused window does nothing.
The `notify` TOML key produces a Notice action, which displays NUL-free text for
three seconds without taking keyboard focus.
The overlay renders two lines of forty characters, using uppercase Latin
letters, digits and punctuation; unsupported characters appear as spaces.
A later notice replaces the previous one. Secret confirmation overlays take
priority over ordinary notices.

Quit requests controller release and compositor shutdown:

```json
{"type":"quit"}
```

Actions run in list order. Permitted actions before Quit run; after a permitted
Quit, further plugin actions are ignored, including later lines and plugins.
Shutdown happens when the event loop next checks the Quit request.

A plugin may send actions without a request:

```json
{"jsonrpc":"2.0","method":"actions","params":{"actions":[{"type":"key","key":"space"}]}}
```

These notices need no event subscription or application match, but require
completed initialization. They have no id, response or button deadline.
They are independent actions and never complete a pending request.
Notice actions are reserved for the core and are dropped from plugins.
Every action requires its matching manifest permission: `key`, `text`, or
`quit`. The core rejects the entire batch if any action lacks permission or
uses a core-only action. No action in a rejected batch is delivered. Permissions restrict
protocol actions; they do not sandbox the process or restrict its OS access.
Unsolicited actions target focus when applied. A focus change settles pending
request decisions as declines and releases their fallback actions once.

Secret references are objects shaped as `{"entry":"mail"}`. A core context
may bind one to a controller button; the compositor asks for fresh confirmation
before lookup and delivery. Plugin-originated requests are rejected and there
is no plugin permission for secret access. Never put secret contents in text,
events, actions, plugin messages or logs.

## Manifest and installation

Place manifests directly in `<context-directory>/plugins/`. Start the
compositor as `tehuti <context-directory> <application> [arguments...]`.
See the [manifest schema](manifest.schema.json). Required fields:

```json
{"executable":"./example.py","app_ids":["test"],"events":["button","focus"],"start":"always","permissions":["key"]}
```

`id`, `author` and `security_contact` are optional strings or null.
Unknown fields are rejected. Manifests have the same 65,536-byte limit.
Executable and application ids must not be blank; whitespace is not trimmed.
Empty lists and duplicate entries are allowed. Events are `button` and `focus`;
permissions are `key`, `text` and `quit`.

Only direct entries with the case-sensitive extension `.json` are read;
there is no recursive search or glob expansion. Manifest symlinks and
nonregular files are rejected. Bad manifests are logged and skipped separately.
Executable paths may be absolute or relative to the plugin directory, including
`..`; they are not confined to that directory. A relative context directory is
resolved from the compositor's working directory. The child inherits that
working directory and environment, including the desktop's `WAYLAND_DISPLAY`;
it is not given the nested application's socket. No shell expands the path.
Make the program executable and use a shebang. Install its interpreter yourself.

`start` is `always` (launch during setup) or `on_focus` (launch on the first
matching focus, then keep running after focus changes). Empty application-id
lists never match, so an `on_focus` plugin with such a list never starts.
Once started, a plugin remains supervised after focus changes.
Failed launches, process exits, stdout EOF and pipe failures schedule a restart.

There is no shutdown message or acknowledgement. On stdout EOF or a pipe
failure, the session is disabled; an unfinished final line is discarded.
On compositor teardown, child processes are killed and reaped. Plugins should
also exit if stdin reaches EOF. Do not rely on a graceful shutdown interval.

## Supervision

The supervisor checks processes every 10 milliseconds, even without input.
Restart backoff starts at 100 milliseconds, doubles after each failure and
caps at 30 seconds. It resets after 30 seconds of initialized operation without
invalid messages. Initialization alone does not reset backoff.
Each failure logs the executable, reason and restart delay.

A plugin that has not initialized within two seconds is terminated. After
initialization, a plugin that misses three decisions in a row is restarted with
backoff. A reply before its decision deadline resets the consecutive miss count.
The core's shorter decision deadlines still release captured fallback actions
on time. Idle plugins with no outstanding decisions may stay silent.
Request ids continue across restarts, so a new process cannot complete an
earlier process's pending decision.

An isolated malformed or overlong line is dropped independently. Sixteen
invalid lines or rejected action batches without 30 seconds of healthy
initialized operation restart the plugin with backoff.
Valid error replies are declines and do not count as invalid messages.

Each stdout callback reads at most 16 chunks of 4096 bytes. Each process may
send at most 262,144 bytes in a one-second window beginning at launch or the
first read after the preceding window expires. Exceeding this limit terminates
the plugin and schedules a restart. Line storage and the stdin queue each stay
bounded at 65,536 bytes, and outstanding decisions stay bounded at 256.
Reads, writes and process status checks do not block. Terminated children are
reaped outside the compositor loop. The pure core step performs no plugin I/O.

## Example session

This complete session uses the manifest above with both event subscriptions.
Each JSON row is one line on the named pipe; labels are not wire data.
Focus changes to `test` while initialization is pending.

| Direction | Line |
| --- | --- |
| Core → stdin | `{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocol_version":0,"encoding":"json","core_version":"0.0.0"}}` |
| Plugin → stdout | `{"jsonrpc":"2.0","id":0,"result":{"protocol_version":0,"encoding":"json","events":["button","focus"]}}` |
| Core → stdin | `{"jsonrpc":"2.0","method":"focus","params":{"app_id":"test","title":"Example window"}}` |
| Core → stdin | `{"jsonrpc":"2.0","id":1,"method":"button","params":{"name":"east","state":"pressed"}}` |
| Plugin → stdout | `{"jsonrpc":"2.0","id":1,"result":[{"type":"key","key":"tab"}]}` |
| Core → stdin | `{"jsonrpc":"2.0","id":2,"method":"button","params":{"name":"east","state":"released"}}` |
| Plugin → stdout | `{"jsonrpc":"2.0","id":2,"result":[]}` |
| Plugin → stdout | `{"jsonrpc":"2.0","method":"actions","params":{"actions":[{"type":"key","key":"space"}]}}` |
| Core → stdin | `{"jsonrpc":"2.0","method":"focus","params":{"app_id":null,"title":""}}` |

The user holds Guide for three seconds and chooses Log out in the system menu.
No Guide or Tick message is sent; the core requests Quit and terminates the
plugin during teardown.
For a runnable button remapping plugin, see
[east-to-tab.py](../../examples/plugins/east-to-tab.py). No Tehuti library is
required: use your language's JSON library and line input/output.
