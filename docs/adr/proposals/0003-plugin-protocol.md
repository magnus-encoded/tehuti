# 3. Plugin protocol

## Status

Proposed. The owner decides; nothing here is accepted.

## Context

Plugins are separate programs that speak JSON-RPC 2.0 over stdin and stdout (#6). Anyone can write one, in any language. Once plugins are out there, some choices are hard to change. The licence is a separate decision (ADR 0004).

## Principle

The core does the work and handles fast input itself. Plugins only make decisions. A plugin that needs speed tells the core its rules ahead of time (bindings, curves, deadlines), and the core applies them. The core never asks a plugin about each pointer move or scroll step.

## Decisions

**1. One JSON message per line.** Each message is one line of UTF-8 JSON. MCP [1] and niri [2] do the same. Every language can read a line and parse JSON, and a person can test a plugin by typing. Logs go to stderr, never stdout [1]. The core logs and drops a bad line, and limits line length.

- Rejected: `Content-Length` headers as in LSP [3]. Every plugin would need extra code.
- Rejected: binary length prefixes. Not plain text.

**2. An `encoding` field.** The first message carries `encoding`; v0 accepts only `"json"`. A binary format (Protobuf [4]) waits in #24. The weekly benchmark in #23 reopens #24 if JSON takes more than 25% of a plugin round trip at p99. If a round trip is slow but JSON is not the cause, the fix is to move that work into the core, per the principle.

**3. Versions.** The core sends `initialize` first, with `protocol_version` (a whole number from 0), `encoding` and its own version. The plugin replies with the version it speaks, the encoding, and the events it wants. MCP works the same way [5]. The core refuses a version it does not support and logs why.

Within one version:

- The core sends only events a plugin asked for, so new events never reach old plugins.
- New fields are optional; plugins ignore fields they do not know.
- The core rejects unknown fields from plugins and names the field in the error.
- Removing or changing a field means a new version. The core supports the current and the previous version for at least one release.

**4. Requests and notices.** Events that need a decision (`button`, `speech`, single scroll steps) are requests. Each gets one reply [6]: a list of actions, empty for "no". Events that only inform (`focus`) are notices with no reply [6]. A plugin can send its own actions as notices; the core checks them against its permissions.

**5. Timeouts.** Each request has a time limit: about two seconds, shorter for buttons (#1). A late or missing reply counts as "no", and a late reply is dropped. Plugins may reply out of order, and the docs say so. Cancelling a request is not in v0. The tests in #7 cover timeouts.

## Sticky

The shape of `initialize`, because every version must read it. So it stays small: the fields above plus one optional object for later needs.

## Sources

1. MCP transports, stdio: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
2. niri IPC: https://github.com/YaLTeR/niri/wiki/IPC
3. LSP 3.17 base protocol: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
4. Protobuf ProtoJSON format: https://protobuf.dev/programming-guides/json/
5. MCP lifecycle, version negotiation: https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle
6. JSON-RPC 2.0: https://www.jsonrpc.org/specification
