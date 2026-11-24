# Ibis constraints, November 2026

Compared on 2026-11-24 against `origin/main` at `650022471077864192fa2ec516a6e73170e0adaa`. The remote main revision was checked through the repository API. The root checkout differs; current file:line references below refer to that revision.

Baseline: [#132](https://github.com/magnus-encoded/tehuti/issues/132), specified 2026-10-16, including its later slice comment; `docs/research/command-builder/HANDOFF.md`, `speech-level1.md`, `haiku-run-1/report.md`, and both `haiku-run-2` reports. Evidence includes the main history since that date, closed issues, current ADRs, all three crate glossaries, and `docs/protocol/README.md`. Slices #891–#898 remain open. Their acceptance criteria leave host, repository and licence choices open.

## Input routing and pad ownership

**Assumed:** A focused bridge receives pad and speech events. It forwards them to Ibis and replies with no actions. South arrives as Enter. Ibis owns the controller only while focused outside Tehuti.

**Now:** Confirm defaults to Enter, but an active context can replace it (#270, PR #271; `docs/contexts.md:75`). Roles vary by pad family. App level gives shoulders to the application; columns level uses them for navigation. Overlays take input first. Virtual passthrough can retain the physical grab while forwarding selected input (`crates/core/GLOSSARY.md:27`, `:29`, `:35`, `:37`). A plugin handling speech itself must return `handled: true`; an empty action list declines (#536; `docs/protocol/README.md:278`).

**Effect:** Strengthens integration through an application plugin. Weakens fixed A/B and unconditional Enter assumptions. The bridge needs routing and overlay tests. Standalone focus ownership remains separate work.

## Speech pipeline and vocabulary

**Assumed:** One shared recogniser under Tehuti. Ibis must add focused-slot vocabulary hints, confidence fields, utterance ids and optional audio capture. Typed readers compensate for poor shell recognition.

**Now:** Context hotwords and runtime `speech.hints` exist (#526, PR #568). Plugins replace their list as slots change. Context words come first within a shared 64-entry cap (`docs/protocol/README.md:287`). Plugins also set standing, focus-scoped language; sessions can use remote transcription with local fallback (#833, PR #834; protocol `:302`). Finals have priority over queued interims (#831, PR #832; `sidecars/speech/speech.py:394`). The speech reply remains text-based; the proposed confidence/id/corpus contract is not supplied by these features (protocol `:275`; sidecar `:674`).

**Effect:** Removes the need to invent the hint mechanism. Strengthens sharing the sidecar. Does not remove typed readers or corpus work. The old synthetic results remain historical: 14/14 tasks hid effort rising from 40 to 107 per task; hints improved top-one accuracy from 20/60 to 47/60 (`speech-level1.md`). They do not measure today's pipeline.

## Context protocol and verb plugins

**Assumed:** One application bridge uses protocol v0 without core changes.

**Now:** Accepted `docs/adr/0007-plugin-kinds.md:9` separates application plugins from global verb plugins. Global verbs receive speech, not pad events. Application plugins can register focus-scoped verbs, which beat global verbs. Initial speech focus governs routing (#863, PR #865; `docs/protocol/README.md:347`, `:430`, `:657`). Built-in phrases still precede plugin verbs. Contexts support title matching, inherited mappings and unmatched dictation (`docs/contexts.md:53`, `:117`).

**Effect:** Strengthens a plugin boundary. A global verb plugin alone cannot implement the builder. Reserved speech and lifecycle rules qualify the original “core does not change” claim. ADR 0003 still supplies framing and compatibility rules (`docs/adr/0003-plugin-protocol.md:17`, `:24`); use the current protocol document for implemented behaviour.

## On-screen keyboard

**Assumed:** Ordinary terminals are impractical with pad and plain dictation, motivating a dedicated terminal builder.

**Now:** Terminal text entry has a local draft, shell symbols, modifier keys, arrows, paging and submit-with-Enter (#224, PR #306; `crates/compositor/GLOSSARY.md:21`). Y opens it; held Y dictates. A context can pass Y through to its application (`docs/contexts.md:446`, `:480`).

**Effect:** Weakens the claim that there is no usable fallback. Does not supply command grammar, ranked typed candidates or editable command history. Ibis's Y-new-command rule now needs an explicit keyboard policy.

## Session restore and terminals

**Assumed:** The terminal owns PTYs, command output and history. No desktop restore contract is specified.

**Now:** Opt-in restore relaunches installed desktop entries and restores placement after clean exit (#858, PR #859). Saved data contains app ids and positions, never commands (`crates/compositor/src/session_restore.rs:1`, `:22`, `:450`). Closed follow-ups did not finish terminal reconnection: [#877](https://github.com/magnus-encoded/tehuti/issues/877) remains open after PR #878.

**Effect:** Strengthens using an installed app with a stable identity. Does not provide PTY survival or Ibis tree/history persistence. Reopening a terminal must not be mistaken for rerunning its commands.

## Agent access

**Assumed:** Agents cannot see or drive the user's terminal. Ibis needs a private opt-in socket and one semantic Screen for human and agent output.

**Now:** The test driver supplies pad, keys, speech, scene inspection and screenshots (#382, PR #384; `crates/compositor/src/bin/tehuti-drive.rs:12`). It refuses live seat sessions (`crates/compositor/src/driver.rs:62`).

**Effect:** Weakens the integration-test infrastructure gap. Does not remove the live-window socket or semantic Screen requirement. ADR 0001's pure step and property-test rationale remains (`docs/adr/0001-correctness-first.md:15`).

## Desktop and distribution

**Assumed:** Ibis supports standalone stock desktops and Tehuti, with separate speech ownership. Three matching artifacts share release automation.

**Now:** Nested Tehuti runs inside an existing desktop but grabs the controller for its lifetime; a full login session also exists (`README.md:73`, `:94`). Tagged two-architecture releases, nested smoke checks and owner promotion exist (#296, #380; `docs/releases.md:3`). Checksummed, versioned installation supports switching and rollback (`scripts/install-release:48`, `:90`).

**Effect:** Nested use strengthens an intermediate development option. It does not replace focus-bound standalone use (#895). Existing release infrastructure removes basic setup work; terminal/library version parity remains additional work (#898).

## Owner questions and new options

- **Terminal host changes.** A context plus application plugin can extend an existing terminal host. A separate builder window is another option. These are alternatives to a fork, not implemented equivalents. Static mappings alone cannot draw the command tree.
- **Repository location changes.** Those boundaries make workspace components or separate plugin repositories viable alternatives to orphan branches. No move follows automatically.
- **Licensing changes conditionally.** Avoiding a fork removes that host's licence as a necessary constraint. Core MPL, protocol Apache and independently licensed plugins remain the recorded boundaries (`docs/adr/0004-licensing.md:15`, `:24`, `:34`). The owner still chooses and records any new licence decision.
- **A/X remains open.** No later capability settles run-on-add or clear-whole-stage. Role-based confirm and Y's keyboard ownership add integration questions, not answers to those interaction choices.
