# 4. Plugin licence

## Status

Proposed. The owner decides; nothing here is accepted.

## Principle

Owner, 2026-10-12: anyone, including a vendor shipping a closed-source plugin, can write a plugin without asking permission or taking on obligations.

## Decision

Plugins always run as separate programs (#1). They talk to the core only through the public protocol (ADR 0003). The protocol docs under `docs/protocol/`, the schemas and any SDK use MIT or Apache-2.0, apart from the core's licence. The SDK is written against the schema, not the core's Rust types.

Why this works: the FSF says programs that talk over pipes or sockets are normally separate programs. Only an exchange of complex internal data could make them one [1]. A public, documented, versioned schema is the clear separate case.

Rejected:

- In-process plugins: ruled out by #1.
- A licence exception like GCC's [2]: not needed while plugins do not link to core code. It stays available if that changes.

## Sticky

Once vendors ship against the spec, its licence cannot be narrowed. So the spec is permissive from the first release. The owner checks with counsel before inviting vendors. This is not legal advice.

## Sources

1. GPL FAQ, mere aggregation: https://www.gnu.org/licenses/gpl-faq.html#MereAggregation
2. GCC Runtime Library Exception: https://www.gnu.org/licenses/gcc-exception-3.1.html
