# Context generation

Preview a context using an installed desktop entry id or a desktop file path:

```sh
scripts/tehuti-generate-context --application sample.desktop \
  --config-parser scripts/context-parsers/supertux2 --out contexts/sample
```

Add `--write` to save it. `--config FILE` overrides the parser's default input.
The parser is an executable, receives that optional path as its only argument,
and prints an object with `actions` and `commands` string-to-binding objects.
Bindings use names accepted by Tehuti, optionally prefixed by modifiers,
for example `shift+f1`. A parser failure or unsupported binding stops generation.
The bundled parsers report unsupported source key codes and omit those mappings.
Unknown action roles remain comments in the generated block for review.
Conflicting roles for one button stop generation.

`supertux2` reads the SDL2 keymap from the XDG data directory's
`supertux2/config`. It selects the first supported key per control and leaves
joystick bindings alone. `mgba` reads the XDG config directory's
`mgba/config.ini`, preferring `gba.input.QT_K` over `gba.input.SDLK` or
`input.SDLK`. It reads keyboard bindings, including Qt modifiers, and ignores
controller bindings. Shortcut settings in `qt.ini` are outside this parser.
The mGBA keyboard format follows its
[Qt input controller](https://github.com/mgba-emu/mgba/blob/master/src/platform/qt/InputController.cpp).

`Name[nb]` takes precedence over `Name`. Launch actions use the desktop entry's
filename, without running its `Exec` value. Entries supplied by path must also
be installed under that desktop id for Tehuti to launch them. If
`StartupWMClass` is missing, `--probe LOG` reads quoted `app_id` values from
a session log. It requires exactly one distinct id; use a log for that window.
The generator never starts a session to discover an id.

`--features ticker,label,hold,fuzzy` permits the corresponding fields.
`TEHUTI_FEATURES` supplies the default list. `--features ''` clears it.
`--ticker-enabled[=true|false]` overrides ticker permission and its enabled
value; `TEHUTI_TICKER` supplies that override when set (also accepts `1`/`0`).
Without either ticker override, the feature list controls permission and
`Game` in `Categories` controls the enabled value. No features are enabled
by default. `--kind game|tool` overrides category detection; games use held
buttons only when the hold feature is enabled.

This main accepts `hold`, `mods` and `launch`. It rejects `label`, `ticker`,
speech `icon` and `speech_matching`; enable those features only for a binary
that implements them. The generated comment lists launch phrases for Whisper
priming. This main has no context field or sidecar interface to supply a
Whisper priming prompt, so the generator does not add one to the TOML schema.

Only the block from `# generated begin` through `# generated end` is replaced.
Keep root fields and comments before it, and additional TOML tables after it.
Do not repeat a generated table outside the block. An existing file without
exactly one ordered marker pair is refused. The default prints a diff and
creates no files. A changed `--write` saves the original bytes in
`context.toml.bak-gen`, replaces the context atomically, and runs
`tehuti --check-config DIR` if `tehuti --help` advertises the option.
`TEHUTI_BIN` selects the validator executable. Missing or unsupported validators
produce a warning. Validation errors, timeouts and execution failures restore
the original context, or remove a newly created file. An unchanged run writes
nothing and keeps the backup. The live session is never restarted.

`scripts/check` runs the Python command-line tests and compositor tests that
parse both gated-off golden contexts with the current parser and binary.
