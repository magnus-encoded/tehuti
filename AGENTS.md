# Tehuti

## Agent skills

### Issue tracker

GitHub Issues on magnus-encoded/tehuti, via `gh`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context: `GLOSSARY-MAP.md` at the root points to one `GLOSSARY.md` per crate. See `docs/agents/domain.md`.

## Standards

Run `scripts/check` before every commit; CI runs the same gate. Review rules are in `CODING_STANDARDS.md`.

## Decisions

Raise every hard-to-reverse decision in the PR body: describe the decision and its options, finish the rest of the ticket, and open the PR as a draft. The repo owner, @magnus-encoded, writes and accepts every ADR. Never add or edit an ADR, whether in `docs/adr/` or in `crates/<context>/docs/adr/`.
