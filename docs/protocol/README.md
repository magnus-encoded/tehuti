# Plugin protocol v0

A plugin is an executable program. The core starts it with no arguments and
connects its stdin and stdout to pipes. Each message is one UTF-8 JSON line,
including a final newline. Flush stdout after every message. Write logs to
stderr. Lines and queued output are limited to 64 KiB. Invalid or overlong
lines are dropped. Unknown fields from plugins are rejected with the field
name in the log. Ignore unknown fields sent by the core.

## Initialize

The first message from the core is this request:

```json
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocol_version":0,"encoding":"json","core_version":"0.0.0"}}
```

An optional `options` object is reserved for future initialize needs. Reply:

```json
{"jsonrpc":"2.0","id":0,"result":{"protocol_version":0,"encoding":"json","events":["button","focus"]}}
```

Only version 0 and encoding `json` are supported. Other versions or encodings
are refused and logged. Send no actions before initialization completes.
Once initialization completes, a focus subscriber receives the latest focus
notice if focus has already changed. Subscriptions are the intersection of the reply's events and the manifest's
events. Initialize expires after two seconds. A late initialize reply is refused.

## Events

Button decisions are requests:

```json
{"jsonrpc":"2.0","id":1,"method":"button","params":{"name":"east","state":"pressed"}}
```

States are `pressed` and `released`. Button names are `south`, `east`, `north`,
`west`, `left_bumper`, `right_bumper`, `select`, `start`, `guide`, `left_stick`,
`right_stick`, `dpad_up`, `dpad_down`, `dpad_left`, `dpad_right`.
The global south press sends Enter before plugin routing. Guide is reserved
for the escape chord and never reaches plugins. Holding Guide for three
seconds quits regardless of plugin behavior. Tick is an internal event and
never crosses the plugin wire.

Reply once with the same integer id and a list of actions:

```json
{"jsonrpc":"2.0","id":1,"result":[{"type":"key","key":"tab"}]}
```

An empty list means no action. At most 256 button requests may be outstanding per plugin. Excess requests
are dropped. Button requests expire after 250 milliseconds;
a missing or late reply means no action. Late and duplicate replies are
dropped. Replies may arrive out of order. v0 has no cancellation request.

Focus is a notice, with no id and no reply:

```json
{"jsonrpc":"2.0","method":"focus","params":{"app_id":"test","title":"Example window"}}
```

`app_id` is null when no application has focus. Focus notices go to every
running subscriber. Button requests go to the first initialized plugin whose
manifest matches the focused application id. Manifest paths sort by name.
That plugin takes the active context's place, including when it returns an
empty action list. The fixed global mapping still takes priority.

## Actions

Key actions press and release one key in the focused Wayland client:

```json
{"type":"key","key":"tab"}
```

Keys are `enter`, `escape`, `space`, `tab`, `backspace`, `up`, `down`, `left`,
`right`. Text actions deliver Unicode text through enabled text-input-v3,
with keyboard taps as the fallback. Text must not contain NUL:

```json
{"type":"text","text":"Hello ✓"}
```

Quit releases the controller and stops the compositor:

```json
{"type":"quit"}
```

A plugin may also send actions without a request:

```json
{"jsonrpc":"2.0","method":"actions","params":{"actions":[{"type":"key","key":"space"}]}}
```

Every action requires its matching manifest permission: `key`, `text`, or
`quit`. Unpermitted actions are dropped. Actions target the currently focused
client. Secret references are reserved objects shaped as `{"entry":"mail"}`.
They name a secret-store entry; there is no secret-value field or secret
delivery action in v0. Never put secret contents in text, events or logs.

## Manifest and installation

Place JSON manifests in `<context-directory>/plugins/`. The compositor is
started as `tehuti <context-directory> <application> [arguments...]`.
A manifest contains exactly these fields:

```json
{"executable":"./example.py","app_ids":["test"],"events":["button","focus"],"start":"always","permissions":["key"]}
```

Executable paths are absolute or relative to the plugin directory. Make the
program executable and use a shebang for its interpreter. Install that
interpreter yourself. `app_ids` uses exact matches. `events` contains `button`
and/or `focus`. `start` is `always` (start on compositor setup) or `on_focus`
(start at the first matching focus, then keep running). Empty application-id
lists match no application. Executable and application ids must not be blank.
Malformed manifests are logged and skipped independently. Processes are
terminated and reaped when the compositor exits; failed plugins do not restart.

For a button remapping example, subscribe to buttons, match application `test`,
and return a Tab key action for a pressed East button. Release events should
return an empty list. The protocol uses only the language's JSON library and
line input/output; no Tehuti library is required.
