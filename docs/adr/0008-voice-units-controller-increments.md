# 8. Voice gives units, the controller increments

## Status

Accepted

## Context

Tehuti has two inputs, voice and a game controller. Both can reach most actions, so every feature must decide which input does what. One rule makes those choices the same everywhere.

## Decision

Voice gives whole units. The controller increments.

A unit is a whole result: a destination, a sentence, a word, a named state, or a verb on the current context. A step moves relative to where you are: the next item, the next letter, one notch more. Voice takes units; the controller takes steps. Navigation puts you in a context, so a verb within it is a unit.

- **The controller is the discovery path.** Stepping reaches everything without knowing its name. Voice jumps only to units whose names you know.
- **Every unit shows the name voice uses for it.** Stepping past a unit teaches you how to say it, and you can ask to go back to it.
- **Repair is a step, not a repeat.** A doubtful recognition becomes candidate units that the controller steps through. Speaking again replaces the query.
- **Steps never go on voice.** Continuous values and relative moves belong to the controller. A named goal state may be spoken.

The verbs that name units across the system belong to Tehuti, not to contexts. Contexts cannot shadow them.

## Consequences

- This is hard to reverse. Users learn the verbs, and every context depends on the split.
- A phrase that names a step is wrong wherever it appears. An action loses its voice path only when it keeps a controller path.
- A continuous value needs a controller adjuster before it can leave voice.
- Names come from what Tehuti knows, ranked by sound, not from hand-written aliases.
- A signpost whose name differs from the name the matcher uses breaks this rule.
