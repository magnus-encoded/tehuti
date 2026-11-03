# On-screen keyboard and controller input: how it should feel

Owner interview, 2026-11-03, for War Room decision D4. Refs #55, #57, #114. Not an ADR.

This note adds to the decisions already recorded on #55 and in the 2026-10-21 interview (`docs/research/osk-design.md`, untracked). Those stand: console-style grid with the low-height layout, the D-pad moves the key highlight, A types, X deletes, Y opens the keyboard (and is space while it is open), B closes it, and speech while it is open is dictation into the draft row. This interview only settles what was left open.

## Decisions

1. **Holding a D-pad direction repeats, then speeds up.** It waits about 300 ms, then steps about 12 keys per second, and goes faster the longer the direction is held. Reason: the owner chose this.
2. **Left-stick word picking depends on how far the stick is pushed.** A small push steps one word at a time; a full push skips quickly. Reason: the owner chose this.
3. **Repeat delay, rates and stick curve are set in TOML only, hot-reloaded, with no settings slider.** This matches the dim settings. Reason: the owner chose this.
4. **Word suggestions (#114) are not part of this work and may never be built.** Do not build them now. Reason: the owner said "certainly not now, maybe never".
5. **Security service (#57), first version: passwords only.** A password field hides its text, takes no speech, and keeps no draft. Rules for which add-ons may see input, and a lock screen, are out of scope. Reason: the owner chose this.

## Hard to reverse

None. Rates and curves are config values. The narrow #57 scope can be widened later without undoing anything.

## Follow-ups for tickets (not created)

- D-pad hold repeat with acceleration, and graded LS word-stepping, with the TOML keys (decisions 1–3).
- #57: rewrite it as the passwords-only scope and check what already exists against it (decision 5).
- #114: park it or close it as wontfix (decision 4).
