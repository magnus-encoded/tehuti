# Declarative contexts

Start with `tehuti <context-directory> <application> [arguments...]`.
The directory can contain a `context.toml` and child directories with their own
`context.toml`. Files load in path order. The first exact application-id match
selects the active context. Invalid files are logged and skipped.

```toml
app_id = "org.example.App"

[buttons.east]
key = "tab"

[buttons.dpad_up]
key = "up"
```

Button names use snake case, such as `south`, `left_bumper` and `dpad_left`.
Keys are `enter`, `escape`, `space`, `tab`, `backspace`, `up`, `down`, `left`,
`right`, `home`, `end`, `page_up` and `page_down`. A mapping sends a complete key tap on button press. Releases send
nothing. Unknown fields, buttons and keys are rejected. Application ids must
contain a non-whitespace character.

An optional top-level `label = "Emulator"` names the application's windows on
the voice bar. At Columns level it replaces the window title; on the focus
chip it replaces the application id. Agent launcher titles keep their agent
or project name on the focus chip. Labels are trimmed and must contain a
non-whitespace character; NUL is rejected. The first context
for an application id decides its label. The `*` context's label is ignored.
Reloading contexts updates the label without another focus change.

The global layer consumes South presses and sends Enter, unless the active
context maps South; then its mapping, tap or hold, replaces Enter. A South
mapping in the `*` global context keeps Enter. The active context handles
other mapped presses. The fallback discards events that neither handles.
Focus changes select a context by the focused toplevel's current application id.

The escape chord consumes every Guide press and release before context routing.
A context mapping on Guide never fires. Holding Guide for 500 ms opens the
system menu. Holding Guide+Start for three seconds emits Quit once; releasing
either button before then cancels the hold.

Double tapping View (Select) within 300 ms toggles mouse mode. A single
press sends its normal mapping on release. Left stick moves the pointer with
more speed at larger deflections; right stick scrolls. South (A) holds the left
mouse button for dragging, East (B) holds the right button. Other controller
buttons are consumed in mouse mode. Clicks focus the window under the pointer.
A cursor and “MOUSE MODE” notice show the mode. Exiting mouse mode or losing
the controller releases held mouse buttons. Holding Guide+Start for three seconds
still quits in mouse mode.

Speech phrases map to the same key or text actions:

```toml
[speech]
confirm = { key = "enter" }
"insert greeting" = { text = "Hello" }
```

Phrase matching ignores case, surrounding whitespace and trailing punctuation.
The first initialized plugin subscribed to speech for the focused application replaces these mappings.
Set top-level `unmatched_speech = "type"` before any TOML table to type unmatched
speech into the focused window, followed by one space. Mapped phrases still win.
The transcript keeps its original casing and punctuation. Empty or whitespace-only
transcripts do nothing. The option applies only to the focused window's context.
Without the option, unmatched speech shows “No speech mapping” for three
seconds above the client without changing focus. Record (Share) controls push-to-talk
by default. Configured push-to-talk buttons never reach context mappings.

## Favourite desktop launch icons

Set top-level `favourites` in the global context (`app_id = "*"`), before any
TOML table:

```toml
app_id = "*"
favourites = ["reader.desktop", "browser.desktop"]

[buttons]

[speech]
Read = { launch = "reader.desktop" }
Browse = { launch = "browser.desktop" }
```

The list defaults to empty. Favourites with global speech launch mappings appear
first in the wallpaper hint grid, in list order. Other launch hints stay visible.
Entries without a launch phrase are skipped; repeated favourites appear once.
With no application focused, D-pad Right or Down selects the next favourite;
Left or Up selects the previous one. Selection stops at either end. Confirm
launches the selected entry using the connected pad's role (A, Cross, or Nintendo
A). Open overlays keep ownership of the pad. Context reloads update favourites.

## Checking a config

`tehuti --check-config [directory]` parses every `context.toml` and the
`menu.toml` in the directory, `~/.config/tehuti` by default, with the parsers
Tehuti uses. It prints each parse error, unknown fields included, and warns
about mappings that never take effect: a `[buttons]` entry for Guide (the
escape chord takes it), for South in the global context unless it sends plain
Enter (the global layer sends Enter for South wherever the focused context does
not map it; a focused context's own South mapping replaces that Enter), or for
a push-to-talk button, and a context whose `app_id` an earlier file already
claims. Modal screens such as the on-screen keyboard, mouse mode and the
settings pages take buttons only while open and are not reported. It exits 1 when any file has an error, and 0
when only warnings remain. It never starts a session.

### Fuzzy speech matching

Whisper often spells a phrase loosely, such as "Åpne Pokeman Rød" for
"Åpne Pokémon Rød". Fuzzy matching, off by default, gives such a transcript
the action of the one phrase it is close to:

```toml
[speech_matching]
fuzzy = true
```

`fuzzy = true` selects a similarity threshold of 0.8; `false` turns it off.
A numeric value between 0 and 1 sets the threshold; 0 turns it off. A context's
value overrides the global `*` context's. It applies only when no phrase of
the focused or global context matches exactly, and before `unmatched_speech`
types the text. Both phrases are folded into a rough Norwegian sound key:
case, punctuation and accents go, æ ø å become e o a, `hv`, `hj` and `gj`
at a word start become `v` and `j`, c, q, w, z and x become k, k, v, s and ks,
doubled letters merge, and the endings `-ene`, `-et`, `-en`, `-er`, `-e`
and `-t` are dropped from words of four letters or more. Similarity is one
less the Damerau-Levenshtein distance per character of the longer key.

Spaces are also ignored without stripping endings, so "Super Tux" matches
"SuperTux". A bare one-word name can match one word or two joined words
inside a sentence, such as "Hun er supertux"; that score is capped at 0.9.

A phrase also needs:

- five letters or more in both keys, so "Ned" or "Hva" never match loosely;
- with two words or more on both sides, first words that are close, at the
  threshold or within one edit; comparison with spaces also requires close
  remaining words, so "Åpne panseret" cannot ride on "Åpne";
- a lead of 0.05 over every phrase with another action, so a transcript
  between two phrases matches neither;
- a similarity of 0.9 or more when it closes a window, quits, or launches an
  entry whose name contains `to-xfce`, `power`, `shutdown`, `reboot`,
  `logout`, `suspend` or `menu`.

Each fuzzy match is logged as `speech fuzzy matched` with the transcript, the
phrase and the score, for review.

## Reload

Tehuti watches the context directory and its child directories. Saving any
`context.toml` reloads every context once edits pause for 300 ms; no restart is
needed. A successful reload shows “Contexts reloaded”. If any file fails to read
or parse, Tehuti keeps the contexts it had and shows a notice naming the file and
the start of the error; the full error goes to the log. A reload leaves held
buttons and a speech session in progress untouched. Plugins are not reloaded;
they restart as processes.

## Voice bar

Every output has a 32 logical pixel bar along its bottom edge. Client windows,
including fullscreen windows, stay above it. The compositor draws it without
creating a focusable surface. The left chip names the focused application id,
or Desktop when no window has focus. A listening chip appears while Record is
held; interim text replaces it as transcription previews arrive. Release shows
Transcribing, then the routed action becomes a result chip. Results and errors
clear after three seconds. Focus changes cancel capture and clear voice feedback.

The clock sits at the right edge. Context, live text, results and errors use
different dark backgrounds with chevron edges. Text and bar height follow the
output scale. Existing notices keep their overlay and focus behaviour.

## Mapping ticker

The bar can scroll one line listing what the focused window's context maps:
pad buttons, push-to-talk and speech phrases. It sits between the chips and
the clock in small text. It is off by default; the global `*` context or an
application's context turns it on, and the application's choice wins.

```toml
[ticker]
enabled = true
speech = "icons"
```

`speech` is `"all"` (the default), `"icons"` for phrases with an icon only,
or `"none"`. Buttons resolve as routing does: the context's mapping, then the
global context's, then Enter on South. Each entry is a row of symbols drawn
in colour, `trigger → result`. Buttons look as they do on the pad in use: on
a symbol pad a blue cross, red circle, pink square and green triangle on dark
discs, and `L1`, `R1`, `L3`, `Share` and `Options` chips; elsewhere the
lettered discs A (green), B (red), X (blue) and Y (yellow), `LB` and `RB`
chips, and the View and Menu symbols. The D-pad is a small cross with the
pressed arm lit. Keys show as keycaps, such as a boxed `Z`; Enter shows ⏎,
Backspace ⌫ and the arrow keys ←↑→↓, and a chord shows `Ctrl` + `O`. A
`hold = true` key adds a dim `hold`. Push-to-talk buttons show a microphone,
and a passthrough context shows `Pad → app` with its push-to-talk chord.
Phrases with the same action share one entry, such as
`"Lagre" / "Lagre spillet" → 💾`; entries with icons come first.

A phrase can name its icon:

```toml
[speech]
"Lagre spillet." = { key = "f1", mods = ["shift"], icon = "💾" }
```

Without one, closing gets ❌, `toggle-fullscreen` ⛶, launchers whose desktop
entry contains `volume-up`, `volume-down` or `mute` get 🔊, 🔉 or 🔇, Ctrl+S 💾,
Ctrl+O 📂 and Ctrl+P ⏯. The bar draws a bitmap font with pictograms for these
icons and for 📥, ⏸, 🔄 and 🎤, some in their own colour. An icon it cannot
draw shows as a word, such as Save, or as the action, such as `Shift` + `F1`
keycaps.

`{ ticker = "toggle" }` on a button or phrase hides the ticker or shows it
again; hiding lasts across focus changes. The ticker is drawn once per entry
list into a strip, and each step copies a window of it, so scrolling costs
one copy of the ticker's area. It moves 6 pixels every quarter second and
redraws only the bar. Text that fits stays still. While
it is off or hidden, time causes no redraws. Focus changes and context
reloads rebuild the entries and start the scroll from the beginning.

## Keyboard intent

The keyboard opens only from Y or the spoken word "keyboard". Entering app
level or columns level and enabling a text field keep it closed.

In `[keyboard]`, `on_text_field = "button"` (the default) lets Y open
while a text field is active or in a terminal. Without either, Y follows normal
application routing. `"open"` lets Y open even without a text field, and
`"off"` passes Y to the application. These options never open the keyboard
automatically. A context with `app_id = "*"` supplies defaults; the active
context overrides options it supplies. Saying "keyboard" opens in every
application regardless of these options. Y types a space while the keyboard is
open. B closes it; Menu types the draft, presses Enter and keeps it open.
With an empty draft Menu presses only Enter. Closing drops the draft by
default; `keep_draft = true` in `[keyboard]` keeps it between openings.
A different text field still clears it.
The draft row sits below the keys, beside the powerline, and shows a window
of text around the caret or picked word.
The legacy `button` and `open_on_app_level` options remain accepted but
do not change the opening triggers. See `examples/global/context.toml`.

A terminal reports the terminal field purpose or has `terminal = true`.
Its layout adds Esc, Tab, Ctrl, Alt, arrows, paging keys and shell symbols.
Strip keys send the draft then tap the key. Ctrl and Alt latch for the next
key. Menu and send also press Enter and keep the keyboard open. Dictation
into a terminal drops commas and full stops that end a word.

## Level dimming

At the app level, other visible application windows and their popups dim. The
columns level leaves every window bright. One visible window stays bright.
The powerline, notices, launcher, settings and keyboard stay bright.

The global `*` context supplies defaults; each application's context can override
individual values. Context reload applies these values without restarting.

```toml
[dim]
enabled = true
strength = 0.67
fade_ms = 100
```

`strength` is the retained colour fraction, between 0 and 1, rounded to
thousandths. `fade_ms` is the time to full dimming in milliseconds (0 to 65535).
The defaults shown above apply when the values are absent.

The global context accepts `push_to_talk = ['start']` before `[buttons]` to use Menu for push-to-talk. The default is `['record']`; `[]` disables controller push-to-talk. Listed buttons replace their context bindings; Guide+Start still escapes.

## Column widths

The global context maps "Wider." to `switch-preset-column-width` and
"Narrower." to `switch-preset-column-width-back`. These layout actions cycle
the focused column through the width presets forward and backward, wrapping
at either end.

```toml
[speech]
"Wider." = { layout = "switch-preset-column-width" }
"Narrower." = { layout = "switch-preset-column-width-back" }
```

New columns default to one-third of the available width, the narrowest preset.
`LayoutConfig.default_column_width` and per-window rules can override it.

## Pad passthrough

A context with `[pad] passthrough = true` lets the focused application read the
pad directly at the app level. The columns level takes the pad back, so a Guide
tap gives every button its normal mapping, such as the right stick toggling
fullscreen, and another tap returns the pad. Hold View (Select) alone for
500 ms to take the pad back for normal keyboard, window focus and mouse
handling. Release keeps that mode;
another solo hold gives the pad back. Any other button cancels the pending
hold. Changing focused windows restores the context's configured mode, even
between windows with the same application id. Title updates keep the mode.

A short View press still reaches the passthrough application directly.
In normal handling, short presses retain View's single and double tap actions.
Guide stays reserved for Tehuti: hold it for 500 ms to open the system menu;
Guide+Start held for three seconds quits.

The pad's `push_to_talk` list defines a chord in passthrough mode. A chord
containing View cancels the solo hold and records while the complete chord is
held. If View is the only push-to-talk button, capture starts and stops only
on a release before 500 ms. A solo hold never starts capture.

## App level tabs

At app level, the global example maps LB to Shift+Tab and RB to Tab.
Its commented Ctrl+PageUp and Ctrl+PageDown mappings offer another choice.
An application context's button mapping overrides the global mapping.
Tab often moves focus within a page; choose the application's tab shortcuts
in its context when needed.

“Tab left” and “tab right” use the same LB and RB mappings at app level,
including application overrides. While the keyboard is open, these phrases
are dictation. “Left” and “right” in the global example move column focus
at either level. At columns level, LB and RB move column focus.

For applications that read the pad directly, use `[pad] passthrough = true`.
The global example includes this option as commented lines. An application
context overrides the global pad policy; its default is normal handling.
In passthrough mode, app level shoulders stay with the application and the
tab phrases send no keys.
