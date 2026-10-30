# Visual regression tests

Build the binaries with `cargo build --locked -p tehuti --bins`, then run
`scripts/visual-tests target/debug /tmp/visual-results`.
The runner needs Python 3, Pillow and foot. It starts private headless test
sessions at 1280 × 720, scale 1, and stops them when finished. Captures use the
software renderer, bundled shell font and fixed screenshot clock. The blank
client uses an empty foot configuration and has no cursor or terminal text.

The bar, notice, settings and keyboard have committed PNGs in
`tests/visual/goldens`. Every colour channel must differ by at most 3 out of 255;
there is no allowance for displaced or missing pixels. CI uploads captures,
scene JSON, session logs and eightfold amplified diff images as the
`visual-regression` artifact, including on failure.

To update the goldens deliberately, run:

```sh
scripts/visual-tests target/debug /tmp/visual-results --update
```

Review the resulting PNG changes before committing. Updating never changes
pending launcher cases or bypasses scene assertions.

Both launcher goldens are pending #379: appearing at empty start, centring
glyphs and filling the panel must be fixed before accepting those pixels. Both cases still render and save
captures. The window case requires the same focused window to remain mapped.
Launcher layout assertions consume `tehuti-drive scene --json`: ink centres
must be within one physical pixel of their circles, rows must have equal heights
and no gaps, and rows must fill the panel above the bar. These assertions report
pending failures until the cases are enabled. To enable them, remove the two launcher
names from `PENDING`, run the update command, and review their PNGs. Layout
assertions must then run before accepting or comparing launcher goldens.
