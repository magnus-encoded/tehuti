# 7. D-Bus library: zbus

## Status

Accepted

## Context

The secret store ([0006](0006-secret-store.md)) is reached over D-Bus, and later desktop services (notifications, power, media keys) will be too. Tehuti had no D-Bus library (#574, #819).

## Decision

- Tehuti talks D-Bus through zbus (pure Rust), using its blocking calls.
- Only one D-Bus library in the workspace. A specialised crate built on zbus (e.g. oo7) is allowed later, if it saves real code.
- D-Bus calls never run on the compositor thread; a call that waits on the user (store prompt) runs on its own thread.
- Secrets cross the session bus in the `plain` session. The bus is the user's own; the `dh-ietf1024` session needs hand-rolled, weak crypto.
- Rejected: `dbus` (C libdbus, outside Rust's safety checks); `oo7` alone (covers the secret store only).

## Consequences

- About 32 crates enter the build (async-io stack, no second async runtime). Trimming features is follow-up work.
- Tests run against an in-process fake service over a socket pair; no real bus is needed in CI.
- A same-user process that eavesdrops on the bus can read secrets in transit; it could already read them from the store.
