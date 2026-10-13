# 6. Secret store

## Status

Accepted

## Context

Backend + narrow interface open per [0002 L21](0002-secrets-by-reference.md?plain=1#L21) (#2). Users may lack keyboard; Tehuti runs without another desktop environment.

## Decision

- Core: client of freedesktop Secret Service API. Recommended store: KeePassXC (no desktop environment needed; second factor addable).
- Unlock: controller button sequence, ≥12 presses, passed to store as master password. Never stored.
- Lock timing: store or plugin policy. Core only forwards lock requests.
- Matching: core matches spoken text to entry names by sound + spelling; returns top few names, ranked. Plugins never match.
- Permissions:
  - `secret.search`: spoken text → matching entry names.
  - `secret.deliver`: delivery per [0002 L16-17](0002-secrets-by-reference.md?plain=1#L16-L17).
- MVP: these two only.

## Consequences

- Uniform matching across plugins ("night" finds "Knight").
- Store swap (KWallet, GNOME Keyring): no code change.
- 12 presses ≈ short PIN; second factor to follow (passkey, security key, SMS code, or whatever store supports).
- Deferred, each own ADR: list entries; write entries; plugin receives value (amends [0002 L16](0002-secrets-by-reference.md?plain=1#L16)).
