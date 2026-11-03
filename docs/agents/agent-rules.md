# Agent rules

Follow every rule on every task.

- Kill a process by its exact pid, found with `pgrep` or `$!`.
- Run cargo under `nice -n 19`, each lane with its own `CARGO_TARGET_DIR`; lanes that share one corrupt each other's checks.
- Treat the live `~/.config/tehuti` as read-only. Send any change to it through Seshat.
- Keep `*.proptest-regressions` files, `.ignore` and `graft/` out of commits. `scripts/check` fails when they are tracked or staged.

Done when none of these was broken during the task.
