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

Agents never add or edit files in `docs/adr/`. Only the owner accepts an ADR. When a change needs a decision that is hard to reverse, say so in the PR body and stop; the chief of staff turns it into an ADR proposal for the owner.
