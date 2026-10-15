# Wallpapers

Tehuti ships 13 release wallpapers, in codename order from `apteribis` to
`threskiornis`. The current release and earlier releases appear in Settings.
Say “birds of a feather” to unlock every wallpaper. A three-second notice confirms
the unlock. The unlock is saved as `1` in
`$XDG_CONFIG_HOME/tehuti/wallpapers-unlocked`, alongside the saved `wallpaper`
selection. The config directory defaults to `~/.config`.
`TEHUTI_ALL_WALLPAPERS=1` exposes every wallpaper for that process without saving
an unlock.

## Data lookup

For each codename, Tehuti searches `$XDG_DATA_HOME/tehuti` first, then each
`tehuti` directory under `$XDG_DATA_DIRS`, in order. These default to
`~/.local/share` and `/usr/local/share:/usr/share`.
Within each directory, `wallpapers.pak` takes precedence over
`wallpapers/<codename>.png`. Missing or invalid packs and images fall through to
the next candidate. Loose PNGs support development. The legacy `wallpaper.png`
remains a fallback when no saved selection matches.

## Building the pack

Run from the repository:

```sh
cargo run -p tehuti --bin pack-wallpapers -- \
  "$HOME/.local/share/tehuti/wallpapers" \
  "$HOME/.local/share/tehuti/wallpapers.pak"
```

The builder requires all 13 `<codename>.png` files. It validates every PNG before
replacing the destination through a temporary file. Install the pack as
`<data-dir>/tehuti/wallpapers.pak`. Images and packs stay outside git.

## Pack format, version 1

All integers are unsigned, little endian. The header is eight bytes
`TEHUPAK1`, followed by a two-byte entry count. The index contains one record per
image: a one-byte UTF-8 codename length, the codename bytes, and an eight-byte
PNG blob length. Blobs follow the complete index in index order, without padding
or trailing data. Each PNG byte is XORed with `0xa7`.

Readers reject unknown or duplicate codenames, more than 13 entries, empty blobs,
blobs larger than 64 MiB, truncated records and trailing bytes. Image decode
failures fall through to the next candidate. The format uses the existing
pure-Rust PNG decoder. XOR hides image signatures from ordinary file browsers
and image viewers; it provides no secrecy or DRM.

## Spoken phrases

`$XDG_CONFIG_HOME/tehuti/phrases.json` configures the global phrases at startup:

```json
{
  "settings": "pop the hood",
  "unlock_wallpapers": "birds of a feather"
}
```

Omitted fields use their defaults. Matching ignores case and punctuation.
Phrases must contain words and normalize to distinct values. Invalid files log
a warning and use both defaults. These commands take precedence over application
contexts and plugins. The system menu consumes speech while it is open.
