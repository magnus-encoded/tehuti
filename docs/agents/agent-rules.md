# Agent rules

Follow every rule on every task.

- Kill a process by its exact pid. Never use `pkill`, `killall` or a name match.
- Run cargo under `nice -n 19`. Give each lane its own `CARGO_TARGET_DIR`; lanes that share one corrupt each other's checks.
- Treat the live `~/.config/tehuti` as read-only. Send any change to it through Seshat.
- Never commit a `*.proptest-regressions` file, `.ignore`, or anything under `graft/`. `scripts/check` fails when you do.

Done when none of these was broken during the task.
