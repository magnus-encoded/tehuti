# Controller-driven on-screen keyboard

Issue: [#56](https://github.com/magnus-encoded/tehuti/issues/56), part of #55.

## Question

The user has no physical keyboard or mouse, only an Xbox controller and voice. Which on-screen keyboard (OSK) design should Tehuti build for the cases voice handles badly? Those cases are short fields, corrections, and symbols. Which Wayland protocol should it use to deliver text?

## Recommendation

Build the OSK **inside the compositor** as a core-owned overlay. Use a **dual-stick split QWERTY selection keyboard**: the left stick selects keys on the left half and the right stick on the right half. Each bumper commits the key under its stick. Add a word-completion row on the D-pad. Deliver text through the compositor's existing **text-input-v3** path, which already has the keymap-tap fallback. Do not expose `zwp_input_method_v2` or `zwp_virtual_keyboard_v1` in the first version.

Why:

- Dual-stick QWERTY was the fastest condition for first-time users in the only controlled comparison of these layouts. It reached 7.08 WPM against 5.79 WPM for the single-stick alphabetic keyboard, about 22% faster [1]. Users bring their QWERTY knowledge, so there is little to learn [1]. Another bimanual QWERTY study reported 16.4 WPM [4].
- TwoStick, the gesture method closest to Steam's daisywheel, beats a selection keyboard only after training. It reached 14.9 against 12.9 WPM after 20 fifteen-minute sessions, but started slower at 4.3 against 5.6 WPM [2]. Voice already covers bulk text, so the OSK is used little, and that much training will not happen.
- ADR 0003 says the core handles fast input and plugins only make decisions. Stick motion is continuous, so the OSK cannot live in a plugin [5]. A compositor-owned overlay matches the existing notice overlay and the core's input routing [6].
- The compositor already implements text-input-v3 and a keymap-tap fallback for the `text` action [6][7]. An in-compositor OSK can call the same delivery code. input-method-v2 and virtual-keyboard-v1 exist so that a separate client can act as the keyboard [8][9]. Tehuti does not need that, and exposing them would hand keystroke injection to any client [9].

## Requirements

Taken from the issue and the context the task gave:

- **R1 Speed.** Words per minute with little practice, because voice does most of the entry.
- **R2 Errors.** A low error rate, because correcting text by controller is expensive.
- **R3 Controller only.** Works with an Xbox pad and no pointer. It must not use buttons the core reserves: South sends Enter, and Guide and Record (Share) are taken [6].
- **R4 Architecture fit.** Respects ADR 0003: no per-motion plugin round trips, and permissions are checked [5][6].
- **R5 Reuse and licence.** Code that is permissive may be reused; GPL code is a source of ideas only.

## Layout options

Scores run from 0 (fails) to 3 (best).

| Option | R1 Speed | R2 Errors | R3 Controller | R4 Fit | R5 Licence | Total |
| --- | --- | --- | --- | --- | --- | --- |
| A. Single-stick selection keyboard (console default) | 1. 6.48 WPM QWERTY, first use [1]; 5.6 WPM early in [2] | 3. 0.044 error rate [1] | 3 | 3 | 3. Trivial to write | 13 |
| **B. Dual-stick split QWERTY** | **3. 7.08 WPM first use, fastest in [1]; 16.4 WPM in [4]** | 2. 0.054; dual stick adds errors, mostly mirrored-hand errors [1] | 3. Both sticks and both bumpers | 3 | 3. Written from the paper [1] | **14** |
| C. Daisywheel / TwoStick (Steam Big Picture) | 2. 14.9 WPM after 20 sessions, but 4.3 WPM early [2] | 2. 0.86% uncorrected at the end [2] | 3 | 3 | 2. Steam is proprietary; ideas from [2][3] only | 12 |
| D. Radial / EdgeWrite-style gestures | 1. 4.4 to 6.4 WPM for joystick EdgeWrite and the selection keyboard [10] | 1. About twice the error rate of dual-stick QWERTY [1] | 3 | 3 | 2. Gesture alphabet must be learned | 10 |
| E. External OSK client (squeekboard, wvkbd) | 0. Touch and pointer designs, not built for controllers [11][12] | 1. Not measured | 0. Needs a pointer or touch | 1. Needs input-method-v2 or virtual-keyboard-v1 exposed | 0. Both are GPL-3.0 [11][12] | 2 |

Word completion is not a separate layout; it applies to A to D. The controller studies above measured it only for Japanese, where IToNe reached 9.3 to 17.7 WPM [13]. Treat it as an addition to B. Raw speed and error rates come from studies of different populations, so compare scores across rows loosely [1][2][4].

Steam's daisywheel splits the alphabet into eight groups of four. The stick picks a group and a face button picks the letter [3]. Early reviewers found it hard to use quickly [3], which matches the slow start TwoStick showed [2].

## Protocol options

| Option | R1 Speed | R2 Errors | R3 Controller | R4 Fit | R5 Licence | Total |
| --- | --- | --- | --- | --- | --- | --- |
| **P1. Compositor OSK, text-input-v3 commit with keymap fallback (exists)** | 3. No extra client round trip | 3. Unicode commit, no keymap mismatch [6] | 3 | 3. Same path as the `text` action [6][7] | 3. Own code | **15** |
| P2. Expose `zwp_input_method_v2`; OSK is a separate client | 2. One more process hop | 3. Preedit and surrounding text [8] | 3 | 1. Only one input method per seat [8]; a client would read raw stick state | 2. Smithay 0.7 has it [14]; protocol marked experimental [8] | 11 |
| P3. Expose `zwp_virtual_keyboard_v1` | 2 | 1. Keycodes only; text outside the keymap is lost [9] | 3 | 0. Any client could inject keys; the spec asks compositors to restrict it [9] | 2. Smithay 0.7 has it [14] | 8 |

P1 keeps a gap: Tehuti cannot host an external IME such as fcitx5, which non-Latin scripts need. This choice can be reversed later. The trigger is the first user who needs an IME. The action is to add the Smithay `input_method` server [14] as P2, with the OSK becoming its frontend. The text-input-v3 code stays as it is.

The niri tracker reports smithay's virtual-input support sending escape sequences to clients [15]. This is a further reason not to choose P3 now. Treat the report as an idea only; niri is GPL.

## Design sketch

- **Open and close.** A global mapping opens the OSK when a focused text-input-v3 client enables input; a context mapping can also open it. Holding Guide still requests Quit [6].
- **Keys.** Each stick moves a highlight on its half of a QWERTY grid; the left bumper commits the left highlight and the right bumper the right one [1]. West is backspace, north is shift and symbols, and east closes. South keeps sending Enter [6].
- **Completion.** The D-pad picks from three completions drawn from a local word list kept in core. A plugin could later supply candidates as a decision request at key rate, not stick rate, within the 100 ms button budget [5][6].
- **Voice.** Push-to-talk stays on Record [6]. Dictated text lands in the same field, and the OSK is used to correct it.
- **Secrets.** OSK keystrokes into a field are never logged, as protocol text rules require [6]. Secret entries keep their own confirmation path.

## Sources

1. Wilson and Agrawala, "Text Entry Using a Dual Joystick Game Controller", CHI 2006, Table 1: https://www.microsoft.com/en-us/research/wp-content/uploads/2016/10/Text-Entry-Using-a-Dual-Joystick-Game-Controller.pdf
2. Költringer, Isokoski and Grechenig, "TwoStick: Writing with a Game Controller", GI 2007: https://graphicsinterface.org/proceedings/gi2007/gi2007-15 (PDF: https://deco.inso.tuwien.ac.at/wp-content/uploads/2009/05/twostick.pdf)
3. Steam Big Picture daisywheel, hands-on and comparison with TwoStick: https://www.engadget.com/2012-09-11-steam-big-picture-beta-hands-on.html, https://destructoid.com/steams-big-picture-steam-on-your-tv
4. "Bimanual text entry using game controllers: Relying on users' spatial familiarity with QWERTY": https://www.researchgate.net/publication/220055233_Bimanual_text_entry_using_game_controllers_Relying_on_users'_spatial_familiarity_with_QWERTY
5. ADR 0003, plugin protocol: `docs/adr/0003-plugin-protocol.md`
6. Plugin protocol v0, reserved buttons, text action, budgets: `docs/protocol/README.md`
7. Compositor text-input-v3 implementation: `crates/compositor/src/text_input.rs`
8. input-method-unstable-v2: https://wayland.app/protocols/input-method-unstable-v2
9. virtual-keyboard-unstable-v1: https://wayland.app/protocols/virtual-keyboard-unstable-v1
10. Wobbrock et al., "Joystick Text Entry with Date Stamp, Selection Keyboard, and EdgeWrite", CHI 2004: https://dl.acm.org/doi/pdf/10.1145/985921.986126
11. squeekboard, GPL-3.0: https://gitlab.gnome.org/World/Phosh/squeekboard/-/raw/main/COPYING
12. wvkbd, GPL-3.0: https://github.com/jjsullivan5196/wvkbd
13. IToNe, Japanese text input for a dual joystick controller: https://www.researchgate.net/publication/221516652_IToNe_A_Japanese_text_input_method_for_a_dual_joystick_game_controller
14. Smithay 0.7 `wayland::{input_method, text_input, virtual_keyboard}`: https://docs.rs/smithay/0.7.0/smithay/wayland/index.html
15. niri#3394, virtual keyboard sends escape sequences (from `de-survey/REPORT.md`, idea-only): https://github.com/niri-wm/niri/issues/3394
