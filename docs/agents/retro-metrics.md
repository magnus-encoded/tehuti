# Retro metrics

One command per retro item. Run from the repo root; rerun later and compare with the baseline.

| Item | Command | Helped when | Baseline 2026-11-03 |
| --- | --- | --- | --- |
| A | `git log --all --oneline -- '*.proptest-regressions' \| wc -l` | stays at the baseline | 28 |
| B | `tehuti --check-config` on a config with a bad manifest | exits non-zero and names the file | not measurable before the change |
| C | `grep -c "context reload failed" ~/.local/state/tehuti/session.log` against `grep -c "context skipped" ~/.local/state/tehuti/session.log` | "failed" stays flat while "skipped" grows | failed 15, skipped 26 |
| D pkill | `grep -rhoE 'pkill' ~/.claude/projects/-home-magnus-Projects-tehuti/ \| wc -l` | stays at the baseline | 339 |
| D cargo | `grep -rhoE '"command":"[^"]*cargo (test\|check\|clippy\|build)[^"]*"' ~/.claude/projects/-home-magnus-Projects-tehuti/ \| grep -vc 'nice -n 19'` | stays at the baseline | 677 |
| E | `grep -c "Live layout" docs/contexts.md` | is 1 | 0 |

Transcript counts cover all history, so judge D by the increase since the baseline.
