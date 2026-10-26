//! The mapping ticker drawn once per entry list into a small coloured strip, so the
//! bar only copies a window of it as the ticker scrolls.

use tehuti_core::ticker::{Direction, Entry, Piece, Rgb, Shape};

/// The strip's height in logical pixels: a 12 pixel symbol, with text centred on it.
pub const HEIGHT: usize = 12;

/// Where 8x8 text sits inside the strip.
const TEXT_TOP: usize = 2;

/// Logical pixels between pieces of an entry.
const PIECE_GAP: usize = 2;

/// Logical pixels between entries; a dot sits in the middle.
const ENTRY_GAP: usize = 20;

const TEXT: Rgb = Rgb(196, 184, 168);
const DIM: Rgb = Rgb(130, 122, 112);
const CHIP: Rgb = Rgb(84, 80, 78);
const KEY_FILL: Rgb = Rgb(48, 44, 42);
const KEY_EDGE: Rgb = Rgb(160, 154, 148);
const BUTTON: Rgb = Rgb(52, 52, 60);
const DPAD: Rgb = Rgb(96, 92, 90);

/// Pictograms that read better in a colour of their own.
const ICON_COLOURS: [(char, Rgb); 6] = [
    ('\u{1f4be}', Rgb(110, 170, 255)),
    ('\u{274c}', Rgb(240, 84, 72)),
    ('\u{1f3a4}', Rgb(255, 150, 150)),
    ('\u{1f504}', Rgb(120, 220, 140)),
    ('\u{1f4e5}', Rgb(110, 170, 255)),
    ('\u{1f4dc}', Rgb(230, 200, 140)),
];

/// A drawn ticker line in logical pixels, transparent where the bar shows through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Strip {
    width: usize,
    pixels: Vec<Option<Rgb>>,
}

impl Strip {
    /// Draws `entries` in a row with a dot between them.
    #[must_use]
    pub fn paint(entries: &[Entry]) -> Self {
        let width = entries
            .iter()
            .map(entry_width)
            .fold(0usize, usize::saturating_add)
            .saturating_add(ENTRY_GAP.saturating_mul(entries.len().saturating_sub(1)));
        let mut strip = Self {
            width,
            pixels: vec![None; width.saturating_mul(HEIGHT)],
        };
        let mut left = 0usize;
        for (index, entry) in entries.iter().enumerate() {
            if index > 0 {
                let middle = left.saturating_add(ENTRY_GAP / 2);
                strip.fill(middle.saturating_sub(1), 5, 2, 2, DIM);
                left = left.saturating_add(ENTRY_GAP);
            }
            left = strip.entry(entry, left);
        }
        strip
    }

    /// The width in logical pixels.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// The colour at a logical pixel, or `None` where nothing is drawn.
    #[must_use]
    pub fn pixel(&self, column: usize, row: usize) -> Option<Rgb> {
        if column >= self.width || row >= HEIGHT {
            return None;
        }
        self.pixels
            .get(row.saturating_mul(self.width).saturating_add(column))
            .copied()
            .flatten()
    }

    fn set(&mut self, column: usize, row: usize, colour: Rgb) {
        if column >= self.width || row >= HEIGHT {
            return;
        }
        if let Some(pixel) = self
            .pixels
            .get_mut(row.saturating_mul(self.width).saturating_add(column))
        {
            *pixel = Some(colour);
        }
    }

    fn fill(&mut self, left: usize, top: usize, width: usize, height: usize, colour: Rgb) {
        for row in top..top.saturating_add(height) {
            for column in left..left.saturating_add(width) {
                self.set(column, row, colour);
            }
        }
    }

    fn entry(&mut self, entry: &Entry, left: usize) -> usize {
        let left = self.pieces(&entry.trigger, left);
        let left = left.saturating_add(PIECE_GAP);
        self.character('\u{2192}', left, TEXT_TOP, DIM);
        let left = left.saturating_add(8).saturating_add(PIECE_GAP.saturating_mul(2));
        self.pieces(&entry.result, left)
    }

    fn pieces(&mut self, pieces: &[Piece], mut left: usize) -> usize {
        for (index, piece) in pieces.iter().enumerate() {
            if index > 0 {
                left = left.saturating_add(PIECE_GAP);
            }
            left = self.piece(piece, left);
        }
        left
    }

    fn piece(&mut self, piece: &Piece, left: usize) -> usize {
        match piece {
            Piece::Text(text) => self.text(text, left, TEXT_TOP, TEXT),
            Piece::Dim(text) => self.text(text, left, TEXT_TOP, DIM),
            Piece::Glyph { shape, colour } => {
                self.shape(*shape, *colour, left);
                left.saturating_add(HEIGHT)
            }
            Piece::Chip(label) => {
                let width = label_width(label);
                self.rounded(left, width, CHIP);
                let _end = self.text(label, left.saturating_add(2), TEXT_TOP, Rgb::WHITE);
                left.saturating_add(width)
            }
            Piece::Key(label) => {
                let width = label_width(label);
                self.rounded(left, width, KEY_EDGE);
                self.fill(
                    left.saturating_add(1),
                    1,
                    width.saturating_sub(2),
                    HEIGHT.saturating_sub(2),
                    KEY_FILL,
                );
                let _end = self.text(label, left.saturating_add(2), TEXT_TOP, Rgb::WHITE);
                left.saturating_add(width)
            }
            Piece::Icon { icon, fallback } => {
                if crate::pictograms::drawable(icon) {
                    let colour = icon
                        .chars()
                        .find_map(|character| {
                            ICON_COLOURS
                                .iter()
                                .find(|(known, _)| *known == character)
                                .map(|(_, colour)| *colour)
                        })
                        .unwrap_or(Rgb::WHITE);
                    self.text(icon, left, TEXT_TOP, colour)
                } else {
                    self.pieces(fallback, left)
                }
            }
        }
    }

    fn text(&mut self, text: &str, mut left: usize, top: usize, colour: Rgb) -> usize {
        for glyph in crate::pictograms::glyphs(text) {
            self.bitmap(glyph, left, top, colour);
            left = left.saturating_add(8);
        }
        left
    }

    fn character(&mut self, character: char, left: usize, top: usize, colour: Rgb) {
        if let Some(glyph) = crate::pictograms::glyph(character) {
            self.bitmap(glyph, left, top, colour);
        }
    }

    fn bitmap(&mut self, glyph: [u8; 8], left: usize, top: usize, colour: Rgb) {
        for (column, row) in crate::pictograms::lit(glyph) {
            self.set(left.saturating_add(column), top.saturating_add(row), colour);
        }
    }

    /// A full-height box with its corner pixels cut.
    fn rounded(&mut self, left: usize, width: usize, colour: Rgb) {
        self.fill(left, 0, width, HEIGHT, colour);
        let right = left.saturating_add(width).saturating_sub(1);
        for (column, row) in [
            (left, 0),
            (right, 0),
            (left, HEIGHT.saturating_sub(1)),
            (right, HEIGHT.saturating_sub(1)),
        ] {
            if let Some(pixel) = self
                .pixels
                .get_mut(row.saturating_mul(self.width).saturating_add(column))
            {
                *pixel = None;
            }
        }
    }

    /// Draws a 12x12 symbol whose pixels `lit` picks, measured from its centre in half pixels.
    fn symbol(&mut self, left: usize, colour: Rgb, lit: impl Fn(i32, i32) -> bool) {
        for row in 0..HEIGHT {
            for column in 0..HEIGHT {
                if lit(half_from_centre(column), half_from_centre(row)) {
                    self.set(left.saturating_add(column), row, colour);
                }
            }
        }
    }

    fn shape(&mut self, shape: Shape, colour: Rgb, left: usize) {
        match shape {
            Shape::Disc(letter) => {
                self.symbol(left, colour, |horizontal, vertical| {
                    within(horizontal, vertical, 12)
                });
                let ink = if bright(colour) { BUTTON } else { Rgb::WHITE };
                self.character(letter, left.saturating_add(3), 3, ink);
            }
            Shape::Dpad(direction) => {
                self.symbol(left, DPAD, |horizontal, vertical| {
                    horizontal.abs() <= 3 || vertical.abs() <= 3
                });
                self.symbol(left, colour, |horizontal, vertical| match direction {
                    Direction::Upward => horizontal.abs() <= 3 && vertical <= -5,
                    Direction::Down => horizontal.abs() <= 3 && vertical >= 5,
                    Direction::Left => vertical.abs() <= 3 && horizontal <= -5,
                    Direction::Right => vertical.abs() <= 3 && horizontal >= 5,
                });
            }
            Shape::Cross | Shape::Ring | Shape::Square | Shape::Triangle | Shape::View | Shape::Menu => {
                self.symbol(left, BUTTON, |horizontal, vertical| {
                    within(horizontal, vertical, 12)
                });
                self.symbol(left, colour, |horizontal, vertical| {
                    on_button(shape, horizontal, vertical)
                });
            }
        }
    }
}

/// Whether a symbol drawn on a dark button lights a point, in half pixels from the centre.
fn on_button(shape: Shape, horizontal: i32, vertical: i32) -> bool {
    match shape {
        Shape::Cross => {
            horizontal.abs() <= 6 && vertical.abs() <= 6 && horizontal.abs().abs_diff(vertical.abs()) <= 2
        }
        Shape::Ring => within(horizontal, vertical, 9) && !within(horizontal, vertical, 5),
        Shape::Square => (5..=7).contains(&horizontal.abs().max(vertical.abs())),
        Shape::Triangle => {
            let base = vertical == 5 && horizontal.abs() <= 7;
            let side = (-7..=5).contains(&vertical)
                && horizontal
                    .abs()
                    .saturating_mul(2)
                    .abs_diff(vertical.saturating_add(9))
                    <= 2;
            base || side
        }
        Shape::View => {
            let in_front = (-1..=5).contains(&horizontal) && (-1..=5).contains(&vertical);
            let front_edge = in_front && (horizontal.abs_diff(2) == 3 || vertical.abs_diff(2) == 3);
            let back_edge = !in_front
                && (-5..=3).contains(&horizontal)
                && (-5..=1).contains(&vertical)
                && (horizontal == -5 || horizontal == 3 || vertical == -5 || vertical == 1);
            front_edge || back_edge
        }
        Shape::Menu => horizontal.abs() <= 5 && matches!(vertical, -5 | -1 | 3),
        Shape::Disc(_) | Shape::Dpad(_) => false,
    }
}

/// A pixel's distance from the centre of a 12 pixel symbol in half pixels: -11, -9, ..., 11.
fn half_from_centre(index: usize) -> i32 {
    i32::try_from(index)
        .unwrap_or_default()
        .saturating_mul(2)
        .saturating_sub(11)
}

/// Whether a point lies inside a circle around the centre; both are in half pixels.
const fn within(horizontal: i32, vertical: i32, radius: i32) -> bool {
    horizontal
        .saturating_mul(horizontal)
        .saturating_add(vertical.saturating_mul(vertical))
        <= radius.saturating_mul(radius)
}

/// Whether white text would be hard to read on `colour`.
fn bright(colour: Rgb) -> bool {
    let Rgb(red, green, blue) = colour;
    u32::from(red)
        .saturating_mul(299)
        .saturating_add(u32::from(green).saturating_mul(587))
        .saturating_add(u32::from(blue).saturating_mul(114))
        > 180_000
}

fn label_width(label: &str) -> usize {
    crate::pictograms::glyphs(label)
        .count()
        .saturating_mul(8)
        .saturating_add(4)
}

fn pieces_width(pieces: &[Piece]) -> usize {
    pieces
        .iter()
        .map(piece_width)
        .fold(0usize, usize::saturating_add)
        .saturating_add(PIECE_GAP.saturating_mul(pieces.len().saturating_sub(1)))
}

fn piece_width(piece: &Piece) -> usize {
    match piece {
        Piece::Text(text) | Piece::Dim(text) => crate::pictograms::glyphs(text).count().saturating_mul(8),
        Piece::Glyph { .. } => HEIGHT,
        Piece::Chip(label) | Piece::Key(label) => label_width(label),
        Piece::Icon { icon, fallback } => {
            if crate::pictograms::drawable(icon) {
                crate::pictograms::glyphs(icon).count().saturating_mul(8)
            } else {
                pieces_width(fallback)
            }
        }
    }
}

fn entry_width(entry: &Entry) -> usize {
    pieces_width(&entry.trigger)
        .saturating_add(PIECE_GAP)
        .saturating_add(8)
        .saturating_add(PIECE_GAP.saturating_mul(2))
        .saturating_add(pieces_width(&entry.result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tehuti_core::ticker::PadFamily;
    use tehuti_protocol::ButtonName;

    fn colours(strip: &Strip) -> std::collections::HashSet<Rgb> {
        (0..strip.width())
            .flat_map(|column| (0..HEIGHT).filter_map(move |row| strip.pixel(column, row)))
            .collect()
    }

    fn entry(trigger: Piece, result: Piece) -> Entry {
        Entry {
            trigger: vec![trigger],
            result: vec![result],
        }
    }

    #[test]
    fn face_buttons_draw_in_their_pad_colours() {
        for (pad, button) in [
            (PadFamily::Symbols, ButtonName::South),
            (PadFamily::Symbols, ButtonName::East),
            (PadFamily::Symbols, ButtonName::West),
            (PadFamily::Symbols, ButtonName::North),
            (PadFamily::Letters, ButtonName::South),
            (PadFamily::Letters, ButtonName::North),
        ] {
            let Piece::Glyph { colour, .. } = pad.piece(button) else {
                panic!("{pad:?} {button:?} is a symbol");
            };
            let strip = Strip::paint(&[entry(pad.piece(button), Piece::Key("Z".into()))]);
            assert!(
                colours(&strip).contains(&colour),
                "{pad:?} {button:?} shows its colour"
            );
        }
    }

    #[test]
    fn the_strip_is_as_wide_as_its_pieces_and_clear_between_them() {
        let one = Strip::paint(&[entry(Piece::Chip("LB".into()), Piece::Text("Column left".into()))]);
        let two = Strip::paint(&[
            entry(Piece::Chip("LB".into()), Piece::Text("Column left".into())),
            entry(Piece::Chip("LB".into()), Piece::Text("Column left".into())),
        ]);
        assert_eq!(
            two.width(),
            one.width().saturating_mul(2).saturating_add(ENTRY_GAP),
            "entries sit a gap apart"
        );
        assert_eq!(one.pixel(0, 0), None, "chip corners are cut");
        assert_eq!(one.pixel(one.width(), 5), None, "nothing past the end");
    }

    #[test]
    fn an_icon_the_strip_cannot_draw_takes_its_fallback_width() {
        let fallback = vec![Piece::Key("Space".into())];
        let unknown = Strip::paint(&[entry(
            Piece::Text("a".into()),
            Piece::Icon {
                icon: "\u{1f998}".into(),
                fallback: fallback.clone(),
            },
        )]);
        let plain = Strip::paint(&[Entry {
            trigger: vec![Piece::Text("a".into())],
            result: fallback,
        }]);
        assert_eq!(unknown, plain, "the fallback is drawn instead");
    }
}
