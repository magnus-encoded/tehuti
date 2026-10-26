//! 8x8 bitmap glyphs for the bar: font8x8's Latin text plus small pictograms for the
//! ticker's built-in icons, since the bar draws no font with emoji.

use font8x8::UnicodeFonts;

/// One 8x8 glyph row by row; `#` is lit. Bit 0 is the leftmost column, as in font8x8.
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "runs only in constant evaluation, where a bad index fails the build"
)]
const fn bitmap(rows: [&str; 8]) -> [u8; 8] {
    let mut glyph = [0u8; 8];
    let mut row = 0;
    while row < 8 {
        let bytes = rows[row].as_bytes();
        let mut column = 0;
        while column < 8 && column < bytes.len() {
            if bytes[column] == b'#' {
                glyph[row] |= 1 << column;
            }
            column += 1;
        }
        row += 1;
    }
    glyph
}

const PICTOGRAMS: [(char, [u8; 8]); 20] = [
    (
        '\u{1f4be}',
        bitmap([
            "########", "#.####.#", "#.##.#.#", "#.####.#", "#......#", "#.####.#", "#.####.#", "########",
        ]),
    ),
    (
        '\u{1f4c2}',
        bitmap([
            "###.....", "#..####.", "#......#", "########", "#......#", "#......#", "#......#", "########",
        ]),
    ),
    (
        '\u{1f509}',
        bitmap([
            "...#....", "..##....", "####.#..", "####..#.", "####..#.", "####.#..", "..##....", "...#....",
        ]),
    ),
    (
        '\u{1f50a}',
        bitmap([
            "...#..#.", "..##...#", "####.#.#", "####..##", "####..##", "####.#.#", "..##...#", "...#..#.",
        ]),
    ),
    (
        '\u{1f507}',
        bitmap([
            "...#....", "..##....", "####.#.#", "####..#.", "####..#.", "####.#.#", "..##....", "...#....",
        ]),
    ),
    (
        '\u{274c}',
        bitmap([
            "##....##", ".##..##.", "..####..", "...##...", "..####..", ".##..##.", "##....##", "........",
        ]),
    ),
    (
        '\u{26f6}',
        bitmap([
            "###..###", "#......#", "#......#", "........", "........", "#......#", "#......#", "###..###",
        ]),
    ),
    (
        '\u{23ef}',
        bitmap([
            "#.......", "##...#.#", "###..#.#", "####.#.#", "###..#.#", "##...#.#", "#.......", "........",
        ]),
    ),
    (
        '\u{23f8}',
        bitmap([
            "........", ".##..##.", ".##..##.", ".##..##.", ".##..##.", ".##..##.", ".##..##.", "........",
        ]),
    ),
    (
        '\u{1f504}',
        bitmap([
            "..####..", ".#....##", "#....###", "#.......", ".......#", "###....#", "##....#.", "..####..",
        ]),
    ),
    (
        '\u{1f3a4}',
        bitmap([
            "..###...", ".#####..", ".#####..", ".#####..", "#.###.#.", ".#...#..", "..###...", ".#####..",
        ]),
    ),
    (
        '\u{1f4dc}',
        bitmap([
            ".######.", "#......#", ".#.##.#.", ".#....#.", ".#.##.#.", ".#....#.", "#......#", ".######.",
        ]),
    ),
    (
        '\u{1f4e5}',
        bitmap([
            "...##...", "...##...", ".######.", "..####..", "...##...", "#......#", "#......#", "########",
        ]),
    ),
    (
        '\u{23ce}',
        bitmap([
            "........", "......#.", "......#.", "..#...#.", ".##...#.", "#######.", ".##.....", "..#.....",
        ]),
    ),
    (
        '\u{232b}',
        bitmap([
            "........", "..######", ".#.#..#.", "#...##..", "#...##..", ".#.#..#.", "..######", "........",
        ]),
    ),
    (
        '\u{2190}',
        bitmap([
            "........", "..#.....", ".##.....", "#######.", ".##.....", "..#.....", "........", "........",
        ]),
    ),
    (
        '\u{2191}',
        bitmap([
            "...#....", "..###...", ".#####..", "...#....", "...#....", "...#....", "...#....", "........",
        ]),
    ),
    (
        '\u{2192}',
        bitmap([
            "........", "....#...", "....##..", "#######.", "....##..", "....#...", "........", "........",
        ]),
    ),
    (
        '\u{2193}',
        bitmap([
            "...#....", "...#....", "...#....", "...#....", ".#####..", "..###...", "...#....", "........",
        ]),
    ),
    (
        '\u{2026}',
        bitmap([
            "........", "........", "........", "........", "........", "........", "#..#..#.", "........",
        ]),
    ),
];

/// Emoji presentation selectors carry no glyph of their own.
const VARIATION_SELECTOR: char = '\u{fe0f}';

/// The bitmap for `character`, or `None` when the bar cannot draw it.
#[must_use]
pub fn glyph(character: char) -> Option<[u8; 8]> {
    PICTOGRAMS
        .iter()
        .find(|(pictogram, _)| *pictogram == character)
        .map(|(_, bitmap)| *bitmap)
        .or_else(|| font8x8::BASIC_FONTS.get(character))
        .or_else(|| font8x8::LATIN_FONTS.get(character))
}

/// Whether the bar can draw every character of `text`.
#[must_use]
pub fn drawable(text: &str) -> bool {
    text.chars()
        .filter(|character| *character != VARIATION_SELECTOR)
        .all(|character| glyph(character).is_some())
}

/// The characters of `text` the bar draws, one glyph each.
pub fn glyphs(text: &str) -> impl Iterator<Item = [u8; 8]> + '_ {
    text.chars()
        .filter(|character| *character != VARIATION_SELECTOR)
        .map(|character| glyph(character).unwrap_or([255, 129, 129, 129, 129, 129, 129, 255]))
}

/// The lit pixels of an 8x8 glyph as (column, row).
pub fn lit(glyph: [u8; 8]) -> impl Iterator<Item = (usize, usize)> {
    glyph.into_iter().enumerate().flat_map(|(row, bits)| {
        (0..8usize).filter_map(move |column| {
            let mask = 1u8
                .checked_shl(u32::try_from(column).unwrap_or_default())
                .unwrap_or_default();
            (bits & mask != 0).then_some((column, row))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_ticker_icon_has_a_pictogram() {
        for action in [
            tehuti_protocol::Action::Close,
            tehuti_protocol::Action::ToggleTicker,
            tehuti_protocol::Action::Layout {
                action: tehuti_protocol::LayoutAction::ToggleFullscreen,
            },
        ] {
            let icon = tehuti_core::ticker::built_in_icon(&action).unwrap();
            assert!(drawable(icon), "{action:?} -> {icon}");
        }
        for icon in [
            "\u{1f4be}",
            "\u{1f509}",
            "\u{1f50a}",
            "\u{1f3a4}",
            "\u{1f4be}\u{fe0f}",
        ] {
            assert!(drawable(icon), "{icon}");
        }
    }

    #[test]
    fn norwegian_letters_draw_and_unknown_emoji_do_not() {
        assert!(drawable("\"Lagre spillet\" -> Bl\u{e5}b\u{e6}r \u{f8}l"));
        assert!(!drawable("\u{1f600}"), "a face has no pictogram");
    }
}
