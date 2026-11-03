# Plugins: purpose, trust and the first plugin

Owner interview, 2026-11-03. Status: **super experimental**. Plugins and everything below may change without notice while we build the first ones.

## Contexts and plugins

- A context is static config for an application: starting window size, inputs, the voice commands likely to come up (given to Whisper as hotwords when dictating into that app).
- A plugin is a program. It is dynamic and reacts to what is happening.
- Plugins may be closed source and written in any language (ADR 0004).
- Community plugins come first, with the owner as first author. Vendor plugins come later.

## Trust

- Nothing runs until the user allows it. Finer permissions come after that.
- Plugins ask in terms Tehuti understands; Tehuti presents them in terms people understand. The prompt's wording is Tehuti's, not the plugin's.
- People think in terms like "you may run", "you may run whenever I start my system", "you may run when I open this kind of app", "Tehuti may share my system state with you". These illustrate the concepts; they are not a spec.
- Tehuti gets one designed permission prompt component, answered from the pad (A allow, B refuse).
- An update asks again only for new permissions.
- Plugins may break; the desktop must not. A failing plugin is restarted a few times, then switched off with a notice.
- Passwords stay hidden from every plugin (#57). #444 (which add-ons may see input) is settled in direction only: input is a sensitive permission the user grants.

## Install and contexts

- Plugins come from a folder for now; a store may come later.
- A context can name a plugin. If the plugin is missing, the context still loads and a notice says so.
- A context has a section for each plugin's own options. Well-behaved plugins stay in their section; a plugin may sometimes need to reach outside it.

## Open until real plugins exist

Whether a plugin can explain its request in its own words, when it asks (up front or at first need), what to do when the vocabulary lacks a word, how app kinds are decided, and the permission vocabulary itself. The vocabulary will live in the manifest and protocol, so fixing it is an owner decision.

## First plugin: WoW chat router

Without it the game cannot be played without a keyboard. Gameplay stays on the controller through the game's own gamepad support.

It is a plugin rather than a context because it shows channel awareness on the powerline.

v1:

1. A chat mode, like a vim mode. "Chat with party" (party, guild, say, raid, battleground) stays on until switched; every utterance dictates into that channel.
2. The current channel shows on the powerline.
3. "Switch channel" lets D-pad up and down step through channels on the powerline. No graphical overlay in v1.
4. Dictation is sent with A and cancelled with B.
5. A pad button pauses dictation.
6. A few voice actions ("open spells", "open bags", "open map") read the user's key bindings from the WoW config, so they work whatever key is bound.

Later: a channel list picked with the controller, offering a channel switch when a dungeon ends, rebinding by voice through a companion WoW add-on, Wowhead lookups in the browser, Wine settings and add-on management, and the automatic workspace organizer as a second plugin.

Discord: left alone until the launch version is out on 2026-11-04. Then look at how Discord and game chat share the mic.

## Virtual pad (new core component)

Step 3 needs Tehuti to keep some inputs from the game. This is an addition, not a lock-in:

- The virtual pad is a context option backed by a new core component.
- In that mode Tehuti takes complete control of the physical controller and presents a second, virtual controller to the application.
- Physical inputs pass on immediately unless they match an overridden condition.
- That condition can come from a plugin. It is set ahead of time and applied by the core, never decided per press (ADR 0003). The protocol needs a clarification for this.
- It must not add delay.
- Apps whose context does not enable it read the physical pad as before.
