# 3. Plugin protocol: framing, versioning, message shape, licence boundary

## Status

Proposed. The owner decides; nothing here is accepted.

## Context

Issue #6 adds plugin programs that speak JSON-RPC 2.0 on stdio. Plugins are written by the community in any language, often by a coding agent, and vendors may ship closed-source ones. Once plugins exist in the wild, some choices are hard to change. This ADR makes each choice reversible where it can, and names the parts that stay sticky.

## Decision

**Governing principle.** The core provides capabilities and runs continuous input on its own fast path. Plugins only decide, using the capabilities they are given. A plugin that needs low latency declares its constraints ahead of time (bindings, curves, deadlines) and the core executes them. There is no per-event round trip for continuous motion such as pointer movement or axis streams.

**Encoding.** v0 uses newline-delimited JSON (section 1). The `initialize` handshake carries an `encoding` field; v0 accepts only `"json"`. A binary encoding (Protobuf, generated from the same schema and using Protobuf's canonical JSON mapping [13]) is deferred as #24. The weekly benchmark in #23 reopens #24 when JSON encoding and decoding take more than 25% of a plugin round trip at p99; #24 says what to do then. If the round trip is over its 1 ms budget but JSON is under 25%, a binary encoding would not help: the fix is to move that work onto the core's fast path under the principle above.

## 1. Framing on stdio

Decision: **newline-delimited JSON (NDJSON)**, one UTF-8 JSON object per line. MCP's stdio transport [1], niri IPC [2] and Kakoune's JSON UI [3] do the same. Every language has "read a line, parse JSON", so a plugin needs no framing code, and a person can test a plugin by typing into it. Standard JSON encoders escape newlines in strings, so the rule only forbids pretty-printing.

Rejected: `Content-Length` headers as in LSP [4] and DAP [5], which add framing code to every plugin for no gain here; binary length prefixes as in sway IPC [6] or Neovim's MessagePack-RPC [7], which are not plain text.

How it stays reversible: the `encoding` field lets a later version add another encoding without breaking plugins that keep `"json"`.

Risk: a plugin writes a log line to stdout and corrupts the stream. Mitigation: stdout is protocol only and stderr is for logs, as in MCP [1]; the core drops and logs an unparsable line and caps line length.

## 2. Versioning and handshake

Decision: **version negotiation plus subscriptions.** The core sends `initialize` first with `protocol_version` (an integer, starting at 0), `encoding` and the core version. The plugin answers with the version it speaks, the encoding, and the events it subscribes to. This follows MCP, where the server answers with the requested version or another it supports and the client disconnects if it cannot use it [8]; LSP negotiates capabilities only [9]. The core refuses and logs an unsupported version (#6).

How it stays reversible, inside one version:

- The core sends only subscribed events, so a new event type never reaches an old plugin.
- New event fields are optional; plugins ignore fields they do not know.
- The core rejects unknown fields and actions from plugins (`CODING_STANDARDS.md` boundary rule), and names the field in the error.
- A removal or change of meaning bumps the version. The core supports the current and previous version for at least one release.

Sticky: the shape of `initialize` itself, since every version must parse it. Decision: keep it minimal (the four fields above plus an optional object for later capabilities).

## 3. Message shape and timeouts

Decision: **mixed.** Events that need a decision (`button`, `speech`, discrete `axis` steps) are JSON-RPC requests, which carry an `id` and get exactly one response [10]; the result is a list of actions, empty for "declined". Events that only inform (`focus`) are notifications, which get no reply [10]. Plugin-initiated actions (a timer that shows a `notify`) are notifications from the plugin, checked against its permissions. Continuous motion is never a request; the plugin declares bindings and curves and the core applies them (principle above).

The core sets a time limit per request (about two seconds, shorter for buttons per #1), treats a late or missing response as "declined", and drops a late response by id. Cancellation like LSP's `$/cancelRequest` [4] is not in v0.

How it stays reversible: whether an event is a request or a notification is per event type, so moving one type is a version bump that old plugins never see unless they subscribe to it.

Risk: a plugin that answers in order and blocks on slow work makes later events time out. Mitigation: the docs state the time limit and that responses may arrive out of order; the timeout tests in #7 cover it.

## 4. Licence boundary for closed-source plugins

Decision: **separate process plus a permissively licensed protocol spec and SDK.** Plugins are always child processes (#1). The FSF's reading is that pipes and sockets are normally used between separate programs, but that exchanging complex internal data structures could make them one program [11]. A documented, public, versioned schema is the arm's-length case. The docs under `docs/protocol/`, the schemas and any SDK carry MIT or Apache-2.0, separate from the core's licence, and the SDK is written against the schema, not the Rust types.

Rejected: in-process plugins (excluded by #1). A licence exception, like GCC's Runtime Library Exception for code compiled with its runtime libraries [12], stays available if the core ever needs one; it is not needed while plugins do not link to core code.

Sticky: once vendors ship against the spec, its licence cannot be narrowed. Principle (owner, 2026-10-12): anyone, including a vendor shipping a closed-source plugin, can write a plugin without asking permission or taking on obligations. So the spec is published permissively from the first release. The owner confirms with counsel before inviting vendors. This is not legal advice.

## Sources

1. MCP transports, stdio: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
2. niri IPC: https://github.com/YaLTeR/niri/wiki/IPC
3. Kakoune JSON UI: https://github.com/mawww/kakoune/blob/master/doc/json_ui.asciidoc
4. LSP 3.17 base protocol and cancellation: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
5. DAP base protocol: https://microsoft.github.io/debug-adapter-protocol/overview
6. sway-ipc(7): https://github.com/swaywm/sway/blob/master/sway/sway-ipc.7.scd
7. Neovim API, MessagePack-RPC: https://neovim.io/doc/user/api.html
8. MCP lifecycle, version negotiation: https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle
9. LSP initialize request: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#initialize
10. JSON-RPC 2.0: https://www.jsonrpc.org/specification
11. GPL FAQ, mere aggregation: https://www.gnu.org/licenses/gpl-faq.html#MereAggregation
12. GCC Runtime Library Exception: https://www.gnu.org/licenses/gcc-exception-3.1.html
13. Protobuf ProtoJSON format: https://protobuf.dev/programming-guides/json/
