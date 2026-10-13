# Clean-room spec: niri scrollable-tiling behaviour

Issue: #50 (part of #49).

## Clean-room statement

This spec was written without opening, fetching, searching or reading any niri source code. Every statement comes from the user-facing prose sources listed below. No source file, code view, code search result or quoted code was used.

## Sources

Every URL consulted while writing this spec:

- [S1] Layout config reference: https://niri-wm.github.io/niri/Configuration%3A-Layout
- [S2] Wiki home and navigation: https://niri-wm.github.io/niri/
- [S3] Workspaces: https://niri-wm.github.io/niri/Workspaces.html
- [S4] Fullscreen and Maximize: https://niri-wm.github.io/niri/Fullscreen-and-Maximize.html
- [S5] Getting Started (default bindings): https://niri-wm.github.io/niri/Getting-Started.html
- [S6] Key bindings config: https://niri-wm.github.io/niri/Configuration%3A-Key-Bindings.html
- [S7] Design principles: https://niri-wm.github.io/niri/Development%3A-Design-Principles.html
- [S8] Tabs: https://niri-wm.github.io/niri/Tabs.html
- [S9] Window rules: https://niri-wm.github.io/niri/Configuration%3A-Window-Rules.html
- [S10] Animations: https://niri-wm.github.io/niri/Configuration%3A-Animations.html
- [S11] Outputs: https://niri-wm.github.io/niri/Configuration%3A-Outputs.html
- [S12] Miscellaneous config: https://niri-wm.github.io/niri/Configuration%3A-Miscellaneous.html
- [S13] Input config: https://niri-wm.github.io/niri/Configuration%3A-Input.html
- [S14] Gestures: https://niri-wm.github.io/niri/Gestures.html
- [S15] FAQ: https://niri-wm.github.io/niri/FAQ.html
- [S16] Floating windows: https://niri-wm.github.io/niri/Floating-Windows.html
- [S17] LWN, "A tour of the niri scrolling-tiling Wayland compositor": https://lwn.net/Articles/1025866
- [S18] Release notes, page 1 (v26.04 to v0.1.9): https://github.com/niri-wm/niri/releases
- [S19] Release notes, page 2 (v0.1.8 to v0.1.3): https://github.com/niri-wm/niri/releases?page=2
- Search results pages (used only to locate the URLs above): web searches for the niri layout wiki and the niri README phrasing.

## Conventions

- Each statement is numbered `N<area>.<n>` and cites its sources.
- Rectangles, from largest to smallest, per output:
  - "Output area": the full logical output rectangle.
  - "Usable area": the output area minus layer-shell exclusive zones.
  - "Working area": the usable area minus struts (negative struts enlarge it). [S1]
  - "Tiling area": the working area inset on every side by one outer gap `g`. [S1] [confirm: outer gap equals inner gap]
- "Tile": a window plus its border `b` on each side. Tile height = client height + 2b; tile width = client width + 2b. The focus ring sits outside the tile and takes no space. [S1] [S7]
- Statements tagged **[confirm]** are inferred from prose that implies but does not state the rule. The verifier should confirm them by running niri before turning them into properties.
- Animations are specified only as behaviour: what state the layout reaches, and when input sees it. Curves and timings are out of scope beyond what is listed under NA.

## NS: The strip of columns

- NS.1 Each workspace holds an ordered sequence of columns on a horizontal strip with no fixed right bound. Columns beyond the visible area exist and are reachable by scrolling. [S17] [S18]
- NS.2 Each column holds one or more tiled windows stacked vertically. [S5] [S18 v0.1.9]
- NS.3 Opening a new window never changes the size of any existing window. [S7] [S18] [S19 v0.1.8]
- NS.4 Each output has its own strips. A window never spans or overflows onto an adjacent output. [S17]
- NS.5 Gaps of the configured size separate adjacent columns and separate the strip from the working-area edges. [S1]
- NS.6 Struts shrink the working area like layer-shell exclusive zones. Left and right struts leave the neighbouring column partly visible. Negative struts enlarge the working area, possibly past the output edges. [S1]
- NS.7 Layout coordinates are fractional. Gaps, borders, struts and the view offset may take fractional logical values, rounded to physical pixels per output scale at render time. [S1] [S19 v0.1.7]
- NS.8 A window or popup larger than the working area is aligned to the top-left so its top-left region stays visible. [S7]

## NW: Column widths and presets

- NW.1 A column width is either `proportion p` (fraction of working-area width) or `fixed w` (logical pixels). [S1]
- NW.2 Proportions account for gaps: n columns of `proportion 1/n` exactly fill the working area for any gap size. [S1]
- NW.3 A `fixed` size applies to the window itself. A `proportion` applies to the whole tile, including the border. Once set, sizes are stored as tile sizes that include the border. [S7]
- NW.4 The border shrinks the window inside its tile. The focus ring does not change window geometry. [S1]
- NW.5 Default `preset-column-widths` are 1/3, 1/2 and 2/3. [S1]
- NW.6 `switch-preset-column-width` sets the focused column to the next preset in the list. `switch-preset-column-width-back` steps in the reverse order. [S1] [S5] After the last preset the forward cycle wraps to the first, and the back cycle wraps from the first to the last. When the current width matches no preset, the forward step picks the smallest preset larger than the current width. [confirm both]
- NW.7 `default-column-width` sets the width of a new column. Default is `proportion 0.5` in the shipped config. [S1]
- NW.8 An empty `default-column-width {}` lets the client choose its width: the initial configure carries width 0 and a concrete height. [S1]
- NW.9 `set-column-width "+10%"` and `"-10%"` change the focused column width by 10 percentage points of the working area (default binds Mod+= and Mod+-). [S5]
- NW.10 `maximize-column` sets the column width equivalent to `proportion 1.0`. Gaps and struts still apply, and every window in the column keeps its border. [S4]
- NW.11 `expand-column-to-available-width` grows the focused column to fill the free horizontal space left by the currently visible columns. Columns that were fully visible stay fully visible. [S18 v25.02]
- NW.12 All windows in a non-tabbed column share the column's width. [S18 v0.1.9]
- NW.13 Resizes smaller than or equal to 10 logical pixels are applied without a resize animation. [S10]
- NW.14 Interactive resizing from a column's left edge keeps the right edge fixed. [S19 v0.1.6]
- NW.15 Double-clicking a vertical resize edge makes the column full width. [S19 v0.1.6]

## NH: Window heights within a column

- NH.1 In a column of n >= 2 windows, the sum of the n tile heights plus (n-1) inner gaps equals the tiling-area height (working-area height minus 2g), when the windows' minimum sizes allow it. Each client height is its tile height minus 2b. [S1] [S7] [S18 v0.1.9] [confirm: outer gaps excluded as stated]
- NH.2 With automatic height, a single-window column's tile height equals the tiling-area height (working-area height minus 2g), and its requested client height is that tile height minus 2b. This is the n = 1 case of NH.1. [S1] [S17] [S18 v0.1.9] [confirm: outer gaps applied as in NH.1]
- NH.3 Resizing one window's height in a multi-window column resizes the others proportionally. The last-resized window keeps its requested height. [S18 v0.1.9]
- NH.4 A single-window column may be resized to any height, clamped to the output height. [S18 v0.1.9] [S18 v25.01]
- NH.5 Closing a window in a column gives its height to the remaining windows in that column. [S18 v0.1.9]
- NH.6 `set-window-height "+10%"`/`"-10%"` change the focused window height (default Mod+Shift+= and Mod+Shift+-). `reset-window-height` (Mod+Ctrl+R) returns it to automatic. [S5]
- NH.7 Default `preset-window-heights` are 1/3, 1/2 and 2/3 of the working-area height, gap-aware. `switch-preset-window-height` cycles them forward; `switch-preset-window-height-back` cycles backward. [S1] Wrap at the ends and the choice from an off-preset height follow NW.6. [confirm]
- NH.8 Double-clicking a horizontal resize edge resets the window height to fill the available space. [S19 v0.1.6]
- NH.9 `min-height`/`max-height` window rules bound the heights niri requests. `max-height` constrains an automatically sized window only when it equals `min-height`. The client keeps the final say on its size. [S9]
- NH.10 In a tabbed column, every tab has the same window size, equal to the full column height. [S8]

## NF: Focus and movement

### Across columns

- NF.1 `focus-column-left`/`focus-column-right` move focus to the adjacent column. At the first or last column they do nothing. [S5] [confirm: end behaviour]
- NF.2 `focus-column-first`/`focus-column-last` focus the first or last column. [confirm: no allowed prose source describes this action; verify by running niri]
- NF.3 `move-column-left`/`move-column-right` swap the focused column with its neighbour. Focus stays on the moved column. [S5]
- NF.4 Rule of bindings: the Ctrl variant of a focus action moves the focused column or window in that direction. [S5] [S7]
- NF.5 When a column regains focus, the window that last had focus in it is focused. [S9 `is-active-in-column`]

### Within a column

- NF.6 `focus-window-down`/`focus-window-up` move focus within the column. In a tabbed column they switch the visible tab. [S5] [S8]
- NF.7 `focus-window-top`/`bottom`, `focus-window-down-or-top`, `focus-window-up-or-bottom` and `focus-window-in-column <index>` exist. [S18 v25.02] The following details are not stated by any allowed source: `-or-top`/`-or-bottom` wrap to the other end of the column at the boundary; `<index>` is 1-based; an out-of-range index is a no-op. [confirm all three]
- NF.8 `move-window-down`/`move-window-up` reorder the focused window within its column. [S5]
- NF.9 `focus-window-up-or-column-right` focuses the window above, or the column to the right when already at the top. [S18 v0.1.10]

### Consume and expel

- NF.10 `consume-or-expel-window-left`: if the focused window shares a column, it is expelled into a new column to the left; otherwise it joins the column to its left. The `right` variant mirrors this. Default binds Mod+[ and Mod+]. [S5] [confirm: exact branch rule]
- NF.11 `consume-window-into-column` pulls the next column's window into the focused column. `expel-window-from-column` moves the focused window into its own column. [S8] [confirm: which neighbour is consumed]
- NF.12 Consuming and resizing take effect in the layout immediately, without waiting for the client to commit the new size. [S7]
- NF.13 A window that gets its own column (opening, expel, floating to tiling) takes `default-column-display` (normal or tabbed). [S9]

### Focus-follows-mouse and warp

- NF.14 With `focus-follows-mouse`, hovering a window or output focuses it. [S13]
- NF.15 With `max-scroll-amount=X%`, a hover focus change that would scroll the view by more than X% of the working-area width is suppressed. `0%` allows only focus changes that cause no scroll. [S13] [S19 v0.1.8]
- NF.16 Hover focus never pulls a window onto a workspace that is being switched away from. [S18 v0.1.9]
- NF.17 With `warp-mouse-to-focus`, the cursor moves into a newly focused window along only the axes needed. A hidden cursor stays hidden. [S13]
- NF.18 Mouse-click bindings act on the window focused when the click happened, not the window under the pointer. [S6]

## NWS: Workspaces stacked vertically

- NWS.1 Each output has its own ordered, vertical list of workspaces. [S3] [S17]
- NWS.2 Every output always has exactly one empty unnamed workspace at the bottom of its list. [S3]
- NWS.3 When a window opens on that trailing empty workspace, a new empty workspace is appended below it. [S3]
- NWS.4 An empty unnamed workspace that is not the last one is removed when focus leaves it. [S3]
- NWS.5 With `empty-workspace-above-first`, an output also keeps one empty workspace at the top. [S1]
- NWS.6 Named workspaces persist while empty and can be reordered and moved between outputs. [S3] [S19 v0.1.6]
- NWS.7 `focus-workspace-down`/`up` switch to the adjacent workspace on the focused output (Mod+U/Mod+I). [S5]
- NWS.8 `move-column-to-workspace-down`/`up` and `move-column-to-workspace <n>` move the focused column to that workspace. [S3] [S5]
- NWS.9 `move-workspace-down`/`up` reorder the focused workspace within its output. [S3] [S5]
- NWS.10 Index arguments address the workspace currently at that position on the focused output. Indices are positional and change when workspaces are reordered. [S3]
- NWS.11 With `workspace-auto-back-and-forth`, focusing the already-focused workspace by index returns to the previous workspace, tracked by identity across reorders. Without it, the action does nothing. [S13]
- NWS.12 On a workspace switch, input goes to the destination workspace at once, before the switch animation finishes. [S7]
- NWS.13 Scroll-wheel workspace binds may carry `cooldown-ms`, which drops repeat triggers inside the cooldown. [S6]

## NO: Per-output behaviour

- NO.1 Each output has its own active workspace and its own focused window. The inactive focus-ring colour is visible only with several outputs. [S1]
- NO.2 `focus-monitor-<dir>` focuses the output in that direction (Mod+Shift+H/J/K/L). `move-column-to-monitor-<dir>` moves the focused column there (Mod+Ctrl+Shift+H/J/K/L). [S5]
- NO.3 `move-workspace-to-monitor-<dir>`/`-next`/`-previous` move the whole workspace to another output. [S3]
- NO.4 Output direction is computed from the global positions of outputs. [S11]
- NO.5 When an output disconnects, its workspaces move to another output. Each workspace remembers its original output and returns to it when it reconnects. [S3]
- NO.6 A workspace resets its remembered original output to its current output when a new window appears on it. [S7]
- NO.7 The active workspace of an output is preserved across a disconnect and reconnect. [S18 v0.1.10]
- NO.8 Per-output `layout {}` blocks override top-level layout settings for that output; per-named-workspace blocks do the same for that workspace. [S1] [S11] [S18 v25.11]
- NO.9 Proportional widths and heights are recomputed against the working area of the output the workspace is on. [S1] [confirm: on moving a workspace between outputs]
- NO.10 Floating positions are stored relative to the output and restored when a workspace returns to its original output. [S18 v25.01]

## NM: Fullscreen and maximize

- NM.1 `fullscreen-window` makes niri request a client size equal to the output area, with no border, gaps or struts. The actual client geometry is whatever the client commits and may be smaller (fixed-size clients); NM.3 covers that case. [S4]
- NM.2 A fullscreen window stays a tile in the scrolling layout. Neighbouring columns are reachable by scrolling. [S4] [S7]
- NM.3 A fullscreen window smaller than the output is shown over a solid black backdrop. [S4]
- NM.4 When the view rests on a focused fullscreen window, floating windows and the top layer-shell layer are hidden. The overlay layer stays visible. [S4] [S7]
- NM.5 Focusing away from a fullscreen window keeps it fullscreen. [S7]
- NM.6 A non-tabbed column with more than one window cannot be fullscreen as a whole. A tabbed column can. [S8] [confirm: what happens to a multi-window normal column on fullscreen]
- NM.7 `maximize-window-to-edges` makes the client rectangle equal the usable area (output area minus layer-shell exclusive zones). Struts, gaps and border do not apply, so tile equals client. Layer-shell bars stay visible. [S4] [S18 v25.11]
- NM.8 Fullscreening or maximizing a floating window moves it into the scrolling layout. Undoing it returns the window to the floating layout. [S4] [S7]
- NM.9 Window rules `open-fullscreen`, `open-maximized`, `open-maximized-to-edges` act only on the state requested at the initial configure. Later client requests are not changed by them. [S4] [S9]
- NM.10 `toggle-windowed-fullscreen` tells the client it is fullscreen while its tile keeps its normal size. [S4] [S18 v25.05]
- NM.11 The view position is saved when a window goes fullscreen and restored when it leaves fullscreen, unless focus changed in between. [S7]
- NM.12 Leaving fullscreen does not make windows jump, with or without animations. [S18 v25.11]

## NN: New-window placement

- NN.1 A new tiled window opens as a new column directly to the right of the focused column. [S17] [confirm]
- NN.2 New windows never open inside an existing column. [S15]
- NN.3 The new column takes `default-column-width`, overridden per window by a `default-column-width` rule. A `default-window-height` rule sets the initial height. [S1] [S9]
- NN.4 The new window is focused, except when: an `open-focused false` rule applies; it opens on an output other than the focused one; or it is a dialog whose parent is not the focused window (the KeePassXC example: a dialog parented to an unfocused window is not auto-focused). An `open-focused true` rule focuses the window even in these cases. [S9 #open-focused]
- NN.5 `open-on-output`/`open-on-workspace` place it on that output or named workspace, falling back to the focused one if the target does not exist. [S9]
- NN.6 Windows with a parent, and fixed-size windows, open in the floating layout. `open-floating false` forces tiling. [S16]
- NN.7 Window rules that set opening properties apply once, at the initial configure. [S9]
- NN.8 The position of the focused window on screen does not change when windows open, close or resize to its left. [S7]
- NN.9 The view position is saved when a window opens and restored when it closes, unless focus changed in between. This applies also to windows that were not focused when they opened. [S7] [S18 v0.1.10]
- NN.10 When a window in a column closes, niri waits for the remaining windows to finish resizing before hiding the closed one. [S18 v0.1.9]

## NV: View scrolling and centering

- NV.1 With `center-focused-column "never"` (default), focusing a column that is not fully visible scrolls the view by the minimum amount to bring it fully into view at the nearest edge. [S1] A column wider than the tiling area is aligned so its left edge sits at the tiling-area left edge, consistent with NS.8. [confirm]
- NV.2 With `"always"`, the focused column is always horizontally centred in the working area. [S1]
- NV.3 With `"on-overflow"`, the view centres the focused column only when it cannot be fully visible together with the last-focused column; otherwise it behaves like `"never"`. [S1]
- NV.4 With `always-center-single-column`, a workspace with exactly one column centres it, whatever `center-focused-column` says. [S1]
- NV.5 `center-column` (Mod+C) centres the focused column once. [S5] [S17]
- NV.6 A focus change that keeps the focused column fully visible does not scroll the view under `"never"`. [S1] [S7] [confirm]
- NV.7 A three-finger horizontal touchpad swipe scrolls the view. On release it settles, honouring finger velocity, and does not travel past the first or last column. [S10] [S14] [S18 v0.1.9]
- NV.8 Mod plus middle-drag scrolls the view horizontally, keeping the grabbed point under the cursor. [S14] [S19 v0.1.6]
- NV.9 Dragging a tiled window horizontally by its title bar scrolls the view; vertical drag moves the window. [S18 v25.11]
- NV.10 During drag-and-drop or a tiled-window drag, holding the pointer near an output edge scrolls the view. Dragging a floating window does not. [S14] [S18 v25.02]
- NV.11 The floating layout does not scroll. Floating windows always draw above tiled windows. [S16]

## NA: Animations as behaviour

- NA.1 The layout state an action produces is final at once. Animations only interpolate the rendering toward it, and input targets the final state. [S7]
- NA.2 With animations off, each animation runs with zero duration and ends in the same frame. Final states are identical to those with animations on. [S7] [S19 v0.1.5]
- NA.3 Movement caused by an action uses the animation settings of that action: view movement caused by a resize uses `window-resize`. [S10]
- NA.4 Animated kinds: `workspace-switch`, `window-open`, `window-close`, `horizontal-view-movement`, `window-movement`, `window-resize`, plus UI overlays. `slowdown` multiplies all durations. [S10]
- NA.5 A spring released from a gesture starts with the gesture's velocity. [S10] [S19 v0.1.3]
- NA.6 Opening the overview makes windows interactive at once; closing it returns input to windows before the animation ends. [S7]

## Open questions for the verifier

Behaviour the allowed sources do not state, to be settled by running niri:

1. Which column takes focus after the focused column closes (left neighbour, right neighbour, or the last-focused one).
2. Exact end-of-strip behaviour of `focus-column-left/right` (no-op or wrap).
3. Width restored after `maximize-column` is toggled off.
4. Exact neighbour chosen by `consume-window-into-column`.
5. How proportional widths behave when a workspace moves to an output of a different size.
