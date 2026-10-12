# 2. Secrets by reference

## Status

Accepted

## Context

Speech is heard by anyone nearby and passes through transcription, logs, the event stream and plugins. A user who speaks a password exposes it to all of them.

## Decision

Secrets are referred to by a reference (an entry name in a secret store), never by value. A secret value exists only inside the secret store and the one core effect that delivers it to its destination.

- The protocol has secret references, not secret values. No event or action carries a value.
- Core context mappings can request delivery by reference. Plugin- and speech-originated requests remain denied until a separate permission and trust design authorizes them; neither receives secret values.
- Delivering a secret into a window requires an explicit controller confirmation that names the entry and the target window.
- Secret values are never logged, never placed on the clipboard, and never shown on screen.
- A property test checks that no event, action, log record or plugin message ever contains a secret value from the test store.

The secret store backend is chosen separately; the core talks to it through one narrow interface.

## Consequences

Features that would be simpler with plain values (a plugin filling a login form itself) go through the core instead. The backend can be swapped without touching the protocol.
