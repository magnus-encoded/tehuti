# Plugin protocol v0

A plugin is an executable program. The core starts it with no arguments and
pipes stdin and stdout. Send one UTF-8 JSON object per line, ending with a
newline, and flush stdout. Escape newlines inside strings. Log to stderr.
Each incoming line may contain at most 65,536 bytes before the newline.
The core's output queue holds at most 65,536 bytes, including newlines;
events that do not fit are logged and dropped.

## Validation and errors

Use `"jsonrpc":"2.0"` on every message. Batches are not supported.
Unknown fields at any level, unknown methods, invalid values and malformed
JSON cause the whole line to be logged and dropped, including all its actions.
Unknown fields are named in the log. The next line is parsed independently.
Ignore unknown fields sent by the core.

The core sends no JSON-RPC error responses. Plugin `error` replies are also
logged and dropped; they do not complete a pending request. Use `result:[]`
to decline a button. Valid plugin `initialize`, `button` and `focus` calls
are ignored: plugins send only replies and `actions` notices.

Ids are unsigned integers from 0 through 18446744073709551615, never strings,
fractions or negative numbers. Replies require an id; notices should omit it
(the parser also accepts null on notices). Only the core assigns request ids.

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
session are ignored. The process stays alive until it exits or the compositor
shuts down. It receives no refusal reply and is never restarted.

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
`west`, `left_bumper`, `right_bumper`, `select`, `start`, `guide`, `left_stick`,
`right_stick`, `dpad_up`, `dpad_down`, `dpad_left`, `dpad_right`.
All names and enum values are case sensitive.

Guide presses and releases never reach plugins. Holding Guide for three
seconds requests Quit; releasing early cancels the hold. Other buttons still
route while Guide is held. South presses send Enter and never reach plugins;
South releases can reach them. Tick is internal and never crosses the wire.

Reply once with the same id and a list of actions:

```json
{"jsonrpc":"2.0","id":1,"result":[{"type":"key","key":"tab"}]}
```

An empty list means no action. Button ids start at 1 per process. Echo each id exactly.
Replies may arrive out of order. Late, duplicate and unmatched replies are
ignored. There is no cancellation request. At most 256 requests may be pending
per plugin. Excess requests are dropped. Each deadline is 250 milliseconds
from dispatch, before queuing, not from the plugin's receipt. A missing reply,
an invalid reply or a reply received at or after the deadline means no action.

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

The selected plugin replaces the active context. An empty, invalid, missing
or late reply does not fall through to context or fallback
([ADR 0003 §5](../adr/0003-plugin-protocol.md)). If no plugin qualifies, normal
context routing applies. Global mappings and the escape chord take priority.

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
completed initialization. They have no id, reply or button deadline.
Every action requires its matching manifest permission: `key`, `text`, or
`quit`. The core drops each unpermitted action and keeps the others in order.
An unpermitted Quit does not stop the remaining actions. Permissions restrict
protocol actions; they do not sandbox the process or restrict its OS access.
Actions target focus when applied, even if it changed since the request.

Secret references are reserved objects shaped as `{"entry":"mail"}`.
No v0 message or manifest accepts them. There is no secret lookup, delivery
or permission in v0. Never put secret contents in text, events or logs.

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
Each launch is attempted once. Failed or exited processes never restart.

There is no shutdown message or acknowledgement. On stdout EOF or a pipe
failure, the session is disabled; an unfinished final line is discarded.
On compositor teardown, child processes are killed and reaped. Plugins should
also exit if stdin reaches EOF. Do not rely on a graceful shutdown interval.

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

The user holds Guide for three seconds. No Guide or Tick message is sent;
the core requests Quit and terminates the plugin during teardown.
For a runnable button remapping plugin, see
[east-to-tab.py](../../examples/plugins/east-to-tab.py). No Tehuti library is
required: use your language's JSON library and line input/output.
