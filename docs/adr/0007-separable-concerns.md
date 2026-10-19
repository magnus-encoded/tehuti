# 7. Separable concerns

## Status

Proposed

## Context

Tehuti combines a compositor, window manager, desktop shell, input layer and speech layer. A user who needs one part, and already has a better program for another (niri as compositor, their own shell, another speech engine), must take all of Tehuti or none. An accessibility layer that needs the whole stack reaches fewer people.

## Decision

Tehuti is built as concerns that run independently. Each can be omitted and replaced by another program that speaks its interface.

The concerns: display and compositing, window management, interaction policy, desktop shell, text entry, session integration.

- Concerns depend on each other only through a named interface: a type or a protocol message, never another's fields or internals.
- Each concern runs without the others and reports visibly when a neighbour is missing.
- A change that touches a function considers whether it belongs to another concern. Small moves are made in that change; larger ones become issues. This is a direction, not a rewrite.
- A concern boundary becomes a process boundary only when a real replacement needs it.

## Consequences

The interfaces become a public, versioned surface, like the plugin protocol. Cross-concern shortcuts must go through an interface. Replacing a concern, such as running the interaction layer on niri, becomes configuration instead of a fork. Progress is the shrinking count of cross-concern reaches.
