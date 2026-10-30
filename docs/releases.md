# Release channel

Tags publish GitHub pre-releases. A candidate becomes the latest release only
after a person reviews its smoke screenshots and promotes it.
Installers using `releases/latest`, including `scripts/install-release` without
a tag, receive promoted builds. An explicit tag installs that candidate.

The release workflow builds both architectures and runs a headless nested
session against the packaged x86_64 binary using the matching `tehuti-drive`.
It captures `start-screen.png`, `bar.png`, `launcher.png`, `settings.png` and
`keyboard.png`. The launcher appears over a terminal window. The driver waits
for each scene before capturing it. Successful captures become release assets;
failed runs retain workflow artifacts and a session log. This smoke check uses
isolated runtime and configuration directories and stops its test sessions.

To publish an existing tag, run the release workflow with action `publish`
and that tag. Publishing requires both architecture builds and the smoke job
to pass. Publishing again resets the release to pre-release status.

Review all five PNG assets on the candidate release. Check that the start screen,
bar text, launcher rows, settings controls and keyboard are visible and legible.
Then run the release workflow from main with action `promote` and the candidate
tag, such as `v1.2.0`. Promotion requires all five assets, clears pre-release
status and explicitly marks the release latest. The dispatch records who chose
to promote it. Asset presence checks support the human review; they do not judge
visual quality.

For a local smoke run, build both binaries with `cargo build --locked -p tehuti
--bins`, install `foot` and Python Pillow, then run `scripts/release-smoke target/debug smoke`.
For a custom Cargo target directory, pass its binary directory instead.
Promotion can also run as `scripts/promote-release v1.2.0` with an authenticated
GitHub CLI and the release repository selected.

Pixel comparisons and deliberate golden updates are documented in [Visual regression tests](visual-tests.md).
