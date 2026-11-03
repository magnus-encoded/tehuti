# Retro metrics

One command per retro item. Run from the repo root; rerun later and compare with the baseline.
Set `CLAUDE_PROJECT_LOGS` to the folder holding this project's agent transcripts.

| Item | Command | Helped when | Baseline 2026-11-03 |
| --- | --- | --- | --- |
| A | `git ls-files '*.proptest-regressions' \| wc -l` | stays at the baseline | 0 after #469 (8 on main before it) |
| B | `d=$(mktemp -d); mkdir -p $d/foot/plugins; printf "app_id = 'foot'\n[buttons]\n" >$d/foot/context.toml; tehuti --check-config $d \| grep -c ': warning:'` | is 1 | 0 on main before #471 |
| C | `grep -c "context reload failed" ~/.local/state/tehuti/session.log` against `grep -c "context skipped" ~/.local/state/tehuti/session.log` | "failed" stays flat while "skipped" grows | failed 15, skipped 26 |
| D pkill | `grep -rhoE 'pkill' "$CLAUDE_PROJECT_LOGS" \| wc -l` | stays at the baseline | 339 |
| D cargo | `grep -rhoE '"command":"[^"]*cargo (test\|check\|clippy\|build)[^"]*"' "$CLAUDE_PROJECT_LOGS" \| grep -vc 'nice -n 19'` | stays at the baseline | 677 |
| E | `grep -c "Live layout" docs/contexts.md` | is 1 | 0 |

Transcript counts cover all history, so judge D by the increase since the baseline.
