# 7. Separable concerns

## Status

Proposed

## Context

Tehuti is a window manager, a compositor, a desktop shell, an input layer and a speech layer in one program. Users who need one part may already have a better program for another: a different compositor such as niri, their own shell, a different speech engine. If the parts only work together, they must take all of Tehuti or none of it, and an accessibility layer that needs the whole stack reaches fewer people.

## Decision

Tehuti aims to be a set of concerns that run independently. Each concern can be left out and replaced by another program that speaks the same interface.

The concerns are display and compositing, window management, interaction policy, desktop shell, text entry, and session integration.

- A concern depends on another only through a named interface: a type or a protocol message. It never reaches into the other's fields or internals.
- A concern runs without the others present, and degrades visibly when a neighbour is missing.
- When a change touches a function, the author considers whether it belongs to a different concern, and moves it if the move is small. Larger moves become issues. This is a direction taken over time, not a rewrite.
- A concern boundary becomes a process boundary only when a real replacement needs it. Until then a module boundary is enough.

## Consequences

The interfaces between concerns become the public surface, so they are versioned and documented like the plugin protocol. Some code moves between crates, and some shortcuts that reach across concerns must go through an interface. Replacing a concern, for example running the interaction layer on top of niri, becomes a configuration choice instead of a fork. Progress is measured by how few cross-concern reaches remain, not by a single refactor.
