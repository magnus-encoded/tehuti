# 1. Correctness first

## Status

Accepted

## Context

Tehuti is the only way its user can operate the computer. A core that crashes, loses input, or acts on input the user did not give leaves them with no fallback: no mouse and keyboard to recover with.

## Decision

When correctness competes with speed of delivery, features or performance, correctness wins. Concretely:

- Invalid states are unrepresentable where the type system allows it. Boundaries parse outside input into validated types.
- The core is a pure step function at its centre, so its behaviour can be checked exhaustively by generated input.
- Property-based and state-machine tests are the primary test style. Every bug fix adds the property or saved case that would have caught it.
- Test quality is measured: mutation testing (`cargo-mutants`) runs on the core and protocol crates, and a surviving mutant is treated as a missing test.
- Concurrency in the core is checked with `loom` where shared state exists. Any `unsafe` code is run under Miri and, for small pure functions, proved with Kani.
- Boundary parsers are fuzzed continuously.
- Strict clippy settings and CI gates are kept at least as strict as today; loosening one needs an ADR.

## Consequences

Features arrive slower. Some designs are chosen because they are easier to verify, not because they are the fastest or shortest. Contributors and agents must write properties, not only examples.
