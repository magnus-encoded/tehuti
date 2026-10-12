# 3. Plugin protocol: framing, versioning, message shape, licence boundary

## Status

Proposed. The owner decides; nothing here is accepted.

## Context

Issue #6 adds plugin programs that speak JSON-RPC 2.0 on stdio. Plugins are written by the community in any language, often by a coding agent, and vendors may ship closed-source ones. Once plugins exist in the wild, four choices are hard to change. Each section lists the options, a recommendation and its main risk.

## 1. Framing on stdio

Options:

- **A. Newline-delimited JSON (NDJSON).** One UTF-8 JSON object per line, no embedded newlines. Used by MCP's stdio transport [1], niri IPC [2] and Kakoune's JSON UI [3].
- **B. `Content-Length` headers.** Used by LSP [4] and DAP [5]. Allows embedded newlines and lets a reader size its buffer before reading.
- **C. Binary length prefix.** sway IPC uses a magic string, length and type header [6]; Neovim uses a MessagePack stream [7]. Neither is plain text.

Recommendation: **A**. Every language has "read a line, parse JSON", so a plugin needs no framing code, and a person can test a plugin by typing into it. Standard JSON encoders escape newlines in strings, so the "no embedded newline" rule only forbids pretty-printing. #1 and #6 already state one message per line.

Main risk: a plugin pretty-prints or writes a log line to stdout and corrupts the stream. Mitigation: the docs say stdout is protocol only and stderr is for logs (MCP has the same rule [1]); the core drops an unparsable line, logs it, and caps line length so a runaway writer cannot exhaust memory.

## 2. Versioning and handshake

Options:

- **A. Single integer version, exact match.** Simple; every new event breaks every old plugin.
- **B. Version negotiation plus capabilities (MCP style).** The first message is `initialize` with the protocol version and capabilities; the other side answers with the version it will use or an error [8]. LSP does the same with capability objects and no version number [9].
- **C. Version plus subscription lists, and ignore-unknown rules.** The manifest and the handshake list the events a plugin subscribes to and the actions it may send. Additions are new event or action names, or new optional fields.

Recommendation: **B and C together**. The core sends `initialize` first with `protocol_version` (an integer, starting at 0) and the core version. The plugin answers with the protocol version it speaks and the events it subscribes to. The core refuses and logs an unsupported version (acceptance criterion in #6). Inside one version:

- The core sends only events the plugin subscribed to, so a new event type never reaches an old plugin.
- A new field in an event is optional; plugins must ignore fields they do not know.
- The core rejects unknown fields and unknown action types from plugins, as the boundary rule in `CODING_STANDARDS.md` requires.
- A change that removes or changes the meaning of anything bumps the version. The core supports the current and previous version for at least one release.

Main risk: the asymmetry (plugins ignore unknown fields, the core rejects them) surprises authors whose plugin sends a field from a newer version to an older core. Mitigation: the rejection names the field, and the plugin learns the core's version in `initialize`.

## 3. Message shape and timeouts

Options:

- **A. Events as notifications, actions as notifications from the plugin.** No ids; the core cannot tell "no answer" from "slow answer", so the timeout rule in #1 cannot be applied per event.
- **B. Events as requests, actions in the response.** JSON-RPC 2.0 requests carry an `id` and get exactly one response [10]. The response result is a list of actions, empty for "declined". This is how LSP answers queries such as completion [4].
- **C. Mixed.** Events that need a decision (`button`, `axis`, `speech`) are requests; events that only inform (`focus`) are notifications. Plugin-initiated actions not tied to an event (a timer that shows a `notify`) are notifications from the plugin, checked against its permissions.

Recommendation: **C**. A request's response says whether the plugin consumed the event, which the routing order in #1 needs (consumed events stop; declined ones fall through to the fallback). The core sets a time limit per request (about two seconds, shorter for buttons per #1) and treats a late or missing response as "declined"; it drops a late response by id. Cancellation follows LSP's `$/cancelRequest` [4] only if a later need appears; not in v0.

Main risk: a plugin that answers in order and blocks on slow work makes every later event time out. Mitigation: the docs state the time limit and that responses may arrive out of order; the timeout tests in #7 cover it.

## 4. Licence boundary for closed-source plugins

Options:

- **A. Separate process plus a permissively licensed protocol spec and SDK.** The FSF's own reading is that programs that communicate over pipes or sockets with data exchanged at arm's length are normally separate programs, and that the line blurs when the exchanged data are complex internal structures [11]. A documented, public, versioned JSON schema is the arm's-length case.
- **B. A plugin exception clause on the core's licence**, as GCC does with its Runtime Library Exception [12]. Only needed if plugins link to core code.
- **C. Plugins loaded into the core process.** Excluded by #1 and makes the derived-work question hard.

Recommendation: **A**. Plugins are always child processes (#1). The protocol docs under `docs/protocol/`, the JSON schemas and any SDK carry a permissive licence (MIT or Apache-2.0) separate from the core's licence, so a vendor can implement the protocol from the docs without touching core code. The core's own licence stays open (#1, Out of Scope); this choice holds for any core licence.

Main risk: if the core later takes a copyleft licence and the SDK is generated from or copies core source, the SDK's licence is in question. Mitigation: the SDK is written against the published schema, and the schema is the source of truth, not the Rust types. This is not legal advice; the owner should confirm with counsel before inviting vendors.

## Sources

1. MCP transports, stdio: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
2. niri IPC: https://github.com/YaLTeR/niri/wiki/IPC
3. Kakoune JSON UI: https://github.com/mawww/kakoune/blob/master/doc/json_ui.asciidoc
4. LSP 3.17 base protocol and cancellation: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
5. DAP base protocol: https://microsoft.github.io/debug-adapter-protocol/overview
6. sway-ipc(7): https://github.com/swaywm/sway/blob/master/sway/sway-ipc.7.scd
7. Neovim RPC (MessagePack-RPC): https://neovim.io/doc/user/api.html
8. MCP lifecycle, version negotiation: https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle
9. LSP initialize request: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#initialize
10. JSON-RPC 2.0: https://www.jsonrpc.org/specification
11. GPL FAQ, plugins and separate programs: https://www.gnu.org/licenses/gpl-faq.html#GPLPlugins and https://www.gnu.org/licenses/gpl-faq.html#MereAggregation
12. GCC Runtime Library Exception: https://www.gnu.org/licenses/gcc-exception-3.1.html
