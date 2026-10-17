<!-- SPDX-License-Identifier: MPL-2.0 -->
# ibis-commands

`step(state, input)` updates a draft and returns effects without I/O. Supply an
`Environment` input with executable names and paths relative to the working folder.
`State::screen()` is the shared structured presentation; its Display implementation
is the agent's text screen. Only `Draft::ready()` constructs a `ReadyInvocation`.

Run `ibis-commands-ctl screen`, `press BUTTON...`, `say TEXT`, `type TEXT` or `reset`.
`TB_HOME` selects the working folder, `TB_STATE` the JSON state file and `TB_LOG`
the input log. Runs use `sh -c`, with a ten second process-group timeout.

Specifications are JSON data in `specs/`. Unknown programs accept raw arguments.
Typing preserves literal values; speech readers decode spoken symbols and rank
candidates against the environment. A whole command at a program slot offers a
parsed tree for acceptance. The run affordance lives in STATUS.

Run properties with `cargo test -p ibis-commands -- --test-threads=1`.
Run step mutations with `cargo mutants -p ibis-commands --file src/machine.rs --re 'in step|replace step'`.
With a nightly toolchain, fuzz the boundary parsers with `cargo fuzz run --fuzz-dir fuzz parsers`.

Licensed under the Mozilla Public License 2.0 (MPL-2.0); see `LICENSE`.

Run `scripts/check` for formatting, Clippy and tests.

Related branches: [Tehuti](https://github.com/magnus-encoded/tehuti/tree/main) and [Ibis](https://github.com/magnus-encoded/tehuti/tree/ibis).
