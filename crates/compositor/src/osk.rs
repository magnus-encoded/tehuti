//! Native-resolution artwork for the on-screen keyboard, docked just above the bar.

use font8x8::UnicodeFonts;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    utils::Transform,
};
use tehuti_core::keyboard::{Key, Keyboard};
use tehuti_protocol::KeyName;

const BACKGROUND: [u8; 4] = [30, 24, 20, 255];
const KEY: [u8; 4] = [88, 65, 47, 255];
const ACTION: [u8; 4] = [65, 50, 38, 255];
const HIGHLIGHT: [u8; 4] = [119, 88, 36, 255];
const PICKED: [u8; 4] = [77, 96, 42, 255];
const INK: [u8; 4] = [239, 235, 229, 255];

/// Logical height of the draft line plus the key rows, each as tall as the bar.
#[must_use]
pub fn logical_height(keyboard: &Keyboard) -> i32 {
    i32::try_from(keyboard.rows().len().saturating_add(1))
        .unwrap_or(i32::MAX)
        .saturating_mul(tehuti_core::bar::HEIGHT)
}

/// Physical top and height on an output, above the bar and never taller than the space left.
#[must_use]
pub fn placement(output_height: i32, scale: i32, keyboard: &Keyboard) -> (i32, i32) {
    let scale = scale.max(1);
    let bar = tehuti_core::bar::HEIGHT.saturating_mul(scale);
    let room = output_height.saturating_sub(bar).max(0);
    let height = logical_height(keyboard).saturating_mul(scale).min(room);
    (room.saturating_sub(height), height)
}

#[derive(Debug, Default)]
pub struct Buffer {
    key: Option<(Keyboard, i32, i32, i32)>,
    buffer: Option<MemoryRenderBuffer>,
}

impl Buffer {
    /// Imports the cached keyboard for either output renderer; `None` while it is closed.
    ///
    /// # Errors
    /// The renderer could not import the raster buffer.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        keyboard: &Keyboard,
    ) -> Result<Option<MemoryRenderBufferRenderElement<R>>, R::Error>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some((buffer, top)) = self.update(output, keyboard) else {
            return Ok(None);
        };
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, f64::from(top)),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .map(Some)
    }

    fn update(&mut self, output: &Output, keyboard: &Keyboard) -> Option<(&MemoryRenderBuffer, i32)> {
        if !keyboard.is_open() {
            self.key = None;
            self.buffer = None;
            return None;
        }
        let mode = output.current_mode()?;
        let scale = output.current_scale().integer_scale().max(1);
        let (top, height) = placement(mode.size.h, scale, keyboard);
        let key = (keyboard.clone(), mode.size.w, height, scale);
        if self.key.as_ref() != Some(&key) {
            self.buffer = raster(keyboard, mode.size.w, height, scale);
            self.key = Some(key);
        }
        self.buffer.as_ref().map(|buffer| (buffer, top))
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "Strip keys get symbols; any other key shows its keysym name"
)]
fn label(key: Key, shifted: bool, symbols: bool) -> String {
    match key {
        Key::Char(character) if shifted => character.to_uppercase().collect(),
        Key::Char(character) => character.into(),
        Key::Text(text) => text.into(),
        Key::Tap(key) => match key {
            KeyName::Tab => "⇥",
            KeyName::Up => "↑",
            KeyName::Down => "↓",
            KeyName::Left => "←",
            KeyName::Right => "→",
            KeyName::Home => "home",
            KeyName::End => "end",
            KeyName::PageUp => "pgup",
            KeyName::PageDown => "pgdn",
            KeyName::Enter => "⏎",
            KeyName::Escape => "esc",
            KeyName::Space => "spc",
            KeyName::Backspace => "⌫",
            other => return other.keysym_name().to_lowercase(),
        }
        .into(),
        Key::Modifier(tehuti_core::keyboard::Modifier::Control) => "ctrl".into(),
        Key::Modifier(tehuti_core::keyboard::Modifier::Alt) => "alt".into(),
        Key::Shift => "⇧".into(),
        Key::Symbols => if symbols { "abc" } else { "?123" }.into(),
        Key::Backspace => "⌫".into(),
        Key::Send => "send".into(),
        Key::CaretLeft => "<".into(),
        Key::CaretRight => ">".into(),
        Key::Space => "space".into(),
    }
}

/// Bitmap for a label character: the vendored font, then the key symbols it lacks.
fn glyph(character: char) -> Option<[u8; 8]> {
    font8x8::BASIC_FONTS
        .get(character)
        .or_else(|| font8x8::LATIN_FONTS.get(character))
        .or_else(|| symbol(character).map(bits))
}

/// Key symbols missing from font8x8, drawn on the same 8x8 grid.
const fn symbol(character: char) -> Option<[&'static str; 8]> {
    Some(match character {
        '←' => [
            "........", "...#....", "..##....", ".######.", "..##....", "...#....", "........", "........",
        ],
        '→' => [
            "........", "....#...", "....##..", ".######.", "....##..", "....#...", "........", "........",
        ],
        '↑' => [
            "...##...", "..####..", ".######.", "...##...", "...##...", "...##...", "...##...", "........",
        ],
        '↓' => [
            "...##...", "...##...", "...##...", "...##...", ".######.", "..####..", "...##...", "........",
        ],
        '⇧' => [
            "...##...", "..#..#..", ".#....#.", "###..###", "..#..#..", "..#..#..", "..####..", "........",
        ],
        '⇪' => [
            "...##...", "..#..#..", ".#....#.", "###..###", "..#..#..", "..####..", "........", "..####..",
        ],
        '⌫' => [
            "........", "...#####", "..#....#", ".#.#.#.#", "#...#..#", ".#.#.#.#", "..#....#", "...#####",
        ],
        '⇥' => [
            "........", "....#..#", "....##.#", "########", "....##.#", "....#..#", "........", "........",
        ],
        '⏎' => [
            "........", ".......#", "..#....#", ".##....#", "########", ".##.....", "..#.....", "........",
        ],
        _ => return None,
    })
}

/// Packs `#` cells into font8x8 rows, where bit zero is the leftmost column.
fn bits(rows: [&str; 8]) -> [u8; 8] {
    rows.map(|row| {
        row.bytes()
            .enumerate()
            .filter(|(_, cell)| *cell == b'#')
            .fold(0u8, |bits, (column, _)| {
                bits | 1u8.checked_shl(u32::try_from(column).unwrap_or(8)).unwrap_or(0)
            })
    })
}

/// Left and right pixel edges of each key, inside a side margin of eight logical pixels.
#[must_use]
pub fn key_spans(keys: &[tehuti_core::keyboard::Slot], width: usize, scale: usize) -> Vec<(usize, usize)> {
    let margin = scale.saturating_mul(8).min(width / 4);
    let inner = width.saturating_sub(margin.saturating_mul(2));
    let total: usize = keys.iter().map(|slot| usize::from(slot.width)).sum();
    let edge = |units: usize| {
        margin.saturating_add(inner.saturating_mul(units).checked_div(total).unwrap_or_default())
    };
    let mut start = 0usize;
    keys.iter()
        .map(|slot| {
            let end = start.saturating_add(usize::from(slot.width));
            let span = (edge(start), edge(end));
            start = end;
            span
        })
        .collect()
}

/// The largest glyph scale, at most twice the output scale, that fits the label inside the key.
#[must_use]
pub fn label_size(text: &str, key_width: usize, scale: usize) -> usize {
    let room = key_width.saturating_sub(scale.saturating_mul(4));
    let per_size = text.chars().count().max(1).saturating_mul(8);
    room.checked_div(per_size)
        .unwrap_or_default()
        .clamp(1, scale.saturating_mul(2).max(1))
}

fn raster(keyboard: &Keyboard, width: i32, height: i32, scale: i32) -> Option<MemoryRenderBuffer> {
    let width_usize = usize::try_from(width).ok().filter(|width| *width > 0)?;
    let height_usize = usize::try_from(height).ok().filter(|height| *height > 0)?;
    let unit = usize::try_from(scale).ok()?;
    let row_height = usize::try_from(tehuti_core::bar::HEIGHT)
        .ok()?
        .saturating_mul(unit);
    let mut canvas = Canvas {
        width: width_usize,
        height: height_usize,
        pixels: vec![0; width_usize.checked_mul(height_usize)?.checked_mul(4)?],
    };
    canvas.rectangle(0, 0, width_usize, height_usize, BACKGROUND);
    draw_draft(&mut canvas, keyboard, row_height, unit);
    for (row, keys) in keyboard.rows().iter().enumerate() {
        let top = row.saturating_add(1).saturating_mul(row_height);
        for (index, (slot, (left, right))) in keys.iter().zip(key_spans(keys, width_usize, unit)).enumerate()
        {
            let colour = if keyboard.highlight() == (row, index) {
                HIGHLIGHT
            } else if let Key::Modifier(modifier) = slot.key
                && keyboard.is_latched(modifier)
            {
                PICKED
            } else if matches!(slot.key, Key::Char(_)) {
                KEY
            } else {
                ACTION
            };
            canvas.rectangle(
                left.saturating_add(unit),
                top.saturating_add(unit),
                right.saturating_sub(left).saturating_sub(unit.saturating_mul(2)),
                row_height.saturating_sub(unit.saturating_mul(2)),
                colour,
            );
            let text = label(slot.key, keyboard.shifted(), keyboard.symbols());
            let glyph = label_size(&text, right.saturating_sub(left), unit);
            let text_width = text.chars().count().saturating_mul(8).saturating_mul(glyph);
            let start = left.saturating_add(right.saturating_sub(left).saturating_sub(text_width) / 2);
            canvas.text(start, top, right, row_height, &text, glyph);
        }
    }
    Some(MemoryRenderBuffer::from_slice(
        &canvas.pixels,
        Fourcc::Argb8888,
        (width, height),
        scale,
        Transform::Normal,
        None,
    ))
}

fn draw_draft(canvas: &mut Canvas, keyboard: &Keyboard, row_height: usize, unit: usize) {
    let glyph = unit.saturating_mul(2);
    let advance = glyph.saturating_mul(8);
    let margin = advance;
    let visible = canvas
        .width
        .saturating_sub(margin.saturating_mul(2))
        .checked_div(advance)
        .unwrap_or_default()
        .max(1);
    let draft = keyboard.draft();
    let caret = draft
        .get(..keyboard.caret())
        .map_or(0, |prefix| prefix.chars().count());
    let skip = caret.saturating_sub(visible.saturating_sub(1));
    if let Some(word) = keyboard.picked_word() {
        let first = draft.get(..word.start).map_or(0, |prefix| prefix.chars().count());
        let length = draft.get(word).map_or(0, |picked| picked.chars().count());
        let left = margin.saturating_add(first.saturating_sub(skip).saturating_mul(advance));
        canvas.rectangle(
            left,
            unit,
            length.saturating_mul(advance),
            row_height.saturating_sub(unit.saturating_mul(2)),
            PICKED,
        );
    }
    let shown: String = keyboard
        .displayed_draft()
        .chars()
        .skip(skip)
        .take(visible)
        .collect();
    canvas.text(margin, 0, canvas.width, row_height, &shown, glyph);
    if keyboard.picked_word().is_none() {
        let left = margin.saturating_add(caret.saturating_sub(skip).saturating_mul(advance));
        canvas.rectangle(
            left,
            unit.saturating_mul(4),
            unit,
            row_height.saturating_sub(unit.saturating_mul(8)),
            INK,
        );
    }
}

struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Canvas {
    fn rectangle(&mut self, x: usize, y: usize, width: usize, height: usize, colour: [u8; 4]) {
        for row in y..y.saturating_add(height).min(self.height) {
            let start = row.saturating_mul(self.width).saturating_add(x).saturating_mul(4);
            let end = row
                .saturating_mul(self.width)
                .saturating_add(x.saturating_add(width).min(self.width))
                .saturating_mul(4);
            if let Some(pixels) = self.pixels.get_mut(start..end) {
                for pixel in pixels.as_chunks_mut::<4>().0 {
                    pixel.copy_from_slice(&colour);
                }
            }
        }
    }

    fn text(&mut self, start: usize, top: usize, end: usize, height: usize, text: &str, size: usize) {
        let advance = 8usize.saturating_mul(size);
        let top = top.saturating_add(height.saturating_sub(advance) / 2);
        for (index, character) in text.chars().enumerate() {
            let left = start.saturating_add(index.saturating_mul(advance));
            if left.saturating_add(advance) > end {
                break;
            }
            let glyph = glyph(character).unwrap_or([255, 129, 129, 129, 129, 129, 129, 255]);
            for (row, bits) in glyph.into_iter().enumerate() {
                for column in 0..8u32 {
                    if bits & (1u8 << column) != 0 {
                        self.rectangle(
                            left.saturating_add(
                                usize::try_from(column).unwrap_or_default().saturating_mul(size),
                            ),
                            top.saturating_add(row.saturating_mul(size)),
                            size,
                            size,
                            INK,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts() -> Vec<Keyboard> {
        let mut letters = Keyboard::default();
        letters.open();
        let mut terminal = Keyboard::default();
        terminal.set_terminal(true);
        terminal.open();
        vec![letters, terminal]
    }

    #[test]
    fn every_key_stays_inside_the_output_with_a_margin() {
        for keyboard in layouts() {
            for width in [640usize, 1280, 1920, 3840] {
                for scale in [1usize, 2] {
                    for keys in keyboard.rows() {
                        for (left, right) in key_spans(keys, width, scale) {
                            assert!(
                                left >= scale.saturating_mul(8) && right <= width - scale * 8 && left < right
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_label_renders_without_a_missing_glyph() {
        for keyboard in layouts() {
            for keys in keyboard.rows() {
                for slot in *keys {
                    for (shifted, symbols) in [(false, false), (true, false), (false, true)] {
                        let text = label(slot.key, shifted, symbols);
                        assert!(
                            text.chars().all(|character| glyph(character).is_some()),
                            "{text} renders"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn labels_fit_their_keys() {
        for keyboard in layouts() {
            for keys in keyboard.rows() {
                for (slot, (left, right)) in keys.iter().zip(key_spans(keys, 1280, 1)) {
                    let text = label(slot.key, false, false);
                    let size = label_size(&text, right - left, 1);
                    assert!(text.chars().count() * 8 * size <= right - left, "{text} fits");
                }
            }
        }
    }
}
