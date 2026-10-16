# Ibis terminal

Run `ibis-tui` in the working folder. Use a powerline-capable font in Kitty.
Y starts a command. Say a command, A accepts the choice, A again runs it.
D-pad browses and moves focus; LB and RB move stages. X deletes, B backs out.
Left stick navigates builder/history; right stick scrolls output, left jumps to
its beginning and right to its end. Page Up, Page Down, Home and End also scroll.
Ctrl-C exits. Commands run synchronously, with combined output and a ten-second
limit. History retains the last 256 KiB of each command's output in memory.

Tehuti types unmatched speech with a trailing space. After 400 ms of quiet,
that text becomes a `say` input. Ordinary text without a trailing space waits
for Enter. Enter on an empty input line accepts or runs. F1–F4 are A/B/X/Y; F5/F6 are LB/RB. F7–F10 are left-stick
up/down/left/right. Arrows are the D-pad. Page Up/Page Down/Home/End are
right-stick up/down/left/right. These keys work through the normal keyboard
path, including clients without text-input support. Context stick mappings repeat every 180 ms beyond half
travel and replace pointer/scroll motion while the app has focus.

Install from this checkout:

```sh
install -Dm755 "${CARGO_TARGET_DIR:-target}"/release/ibis-tui ~/.local/bin/ibis-tui
install -Dm644 dist/contexts/ibis-tui/context.toml ~/.config/tehuti/ibis-tui/context.toml
install -Dm644 dist/applications/ibis-tui.desktop ~/.local/share/applications/ibis-tui.desktop
```

Kitty and GNU coreutils `timeout` must be on PATH. The running Tehuti must
support context stick mappings. Build and install Tehuti through the normal
session workflow to use that support. The commands dependency points to the
local `commands` branch until a release exists.

Add this entry to the global context's existing `[speech]` table (app id `*`):

```toml
"New command" = { launch = "ibis-tui.desktop" }
```

Focused phrases take precedence over global phrases. Unmatched speech types
into Ibis after both phrase tables have been checked. Context reload follows
the normal Tehuti configuration workflow.

The agent socket listens at `$XDG_RUNTIME_DIR/ibis-tui.sock`; `IBIS_TUI_SOCKET`
overrides the path. Startup removes stale sockets and refuses an active socket.
Each JSON line is an argument array using the `ibis-commands-ctl` verbs:
`["screen"]`, `["press", "Y"]`, `["say", "echo hello"]`, `["type", "hello"]`,
or `["reset"]`. Press accepts multiple buttons. Replies are JSON objects with
`screen` containing the CLI screen text, or `error` for an invalid request.
Inputs use the terminal's command runner; reset restores the initial builder
and environment. Connections support multiple lines. Requests are limited to
64 KiB, with at most 16 clients; incomplete lines do not block terminal input.
