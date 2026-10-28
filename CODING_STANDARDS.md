# Coding standards

Reviewers apply these. Lints and `scripts/prose-lint` enforce the mechanical rules; this file holds the judgement calls.

## Where meaning lives

| Kind of knowledge | Home |
| --- | --- |
| What the code does | The code: names, types, small functions |
| Why a design was chosen | An ADR in `docs/adr/` or `crates/<context>/docs/adr/` |
| How it behaves | Tests, named for the behaviour they pin |
| How it changed | Git history |

- Names carry the semantics. A long, exact name beats a short name plus a comment.
- Comments are rare and short: a fact the code cannot express, such as an external constraint, with a link to its source.
- Every text in the repo describes the present. Change history stays in git.
- Domain terms match the context's `GLOSSARY.md`.

## Prose

Applies to code, docs, issues, commits and PRs.

- Plain, direct sentences. State the claim itself.
- Commas, colons or parentheses for asides.
- Concrete verbs and nouns over business vocabulary.

## Issues

One concrete desired change per issue: the observable behaviour after the change, and how to check it is done.

## Pull requests

`Closes #N`, then one to three sentences on how the change answers that issue. CI caps the body at 120 words.

Every ADR change needs owner signoff. The owner applies the `adr-accepted` label to the PR after deciding; CI checks the label and verifies the owner applied it. ADR-only PRs may omit an issue reference when this signoff is present. PRs that also change other files still need an issue reference.

## Robustness of the core

The core keeps running whatever plugins, clients, devices or input do.

- Every fallible operation returns a `Result` and the caller decides. Panics are a bug.
- Input from outside the process (plugin messages, Wayland clients, devices, config files) is validated at the boundary into types that cannot hold invalid states.
- Failures in one plugin, client or device are contained to it, logged, and recovered from.
- `unsafe` needs an ADR and a `// SAFETY:` comment per block.
- Lint exceptions are `#[expect(lint, reason = "...")]` at the narrowest scope.

## Tests

A test states a rule about behaviour. It never computes its expected value by copying the implementation's logic.

- A property asserts a law over generated inputs, such as "a consumed event never reaches a later layer". A state-machine test may compare against a small reference model that is simpler than the code; a copy of the code is not a model.
- Properties of the core step generate the configuration as well as the events, so they hold for every context, mapping or plugin set, not one fixture.
- Tests go through public seams. They never reach into private state, mock internal modules or assert on call order between internal functions.
- Every documented promise has an automated test, except where a spec or ADR says it is checked by hand.
- No test needs a real device, uinput, display or microphone. Core tests drive the pure step with generated events; compositor tests use the synthetic pad.
- Mutation testing measures this: a surviving mutant is a missing test.

## Separable concerns

Tehuti is built so that a part can be swapped out: another window manager or compositor, such as niri, in place of ours, or another shell, input method or speech engine.

- When a change touches a function, ask whether it belongs to a different concern than the file it sits in: display and compositing, window management, interaction policy, desktop shell, text entry, or session integration.
- If it does and the move is small, make it in the same PR as its own commit. If it is large, open an issue and leave the code where it is.
- A concern depends on another only through a named interface: a type or a protocol message, never a shared field or a call into its internals.
- Do not refactor for its own sake, and do not move code in an unrelated PR beyond what the touched function needs.
