//! The on-screen keyboard's pure step: buttons, the left stick, dictation and time in; draft edits and sends out.

use core::ops::Range;
use core::time::Duration;

use tehuti_protocol::{ButtonName, ButtonState};

/// How long a D-pad direction is held before the highlight starts repeating.
pub const REPEAT_DELAY: Duration = Duration::from_millis(400);
/// Time between highlight moves while a D-pad direction stays held.
pub const REPEAT_INTERVAL: Duration = Duration::from_millis(80);
/// Left stick deflection, in thousandths, that picks the next or previous word.
pub const WORD_PICK_DEFLECTION: i16 = 500;
/// The left stick must return inside this deflection before it picks another word.
const WORD_PICK_RELEASE: i16 = 250;
/// Every row spans this many width units.
pub const ROW_UNITS: u8 = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Shift,
    Symbols,
    Backspace,
    Send,
    CaretLeft,
    CaretRight,
    Space,
}

/// One key and its width in [`ROW_UNITS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub key: Key,
    pub width: u8,
}

const fn slot(key: Key, width: u8) -> Slot {
    Slot { key, width }
}

const fn letter(character: char) -> Slot {
    slot(Key::Char(character), 2)
}

pub type Rows = [&'static [Slot]; 3];

const LETTERS: Rows = [
    &[
        slot(Key::Shift, 4),
        letter('q'),
        letter('w'),
        letter('e'),
        letter('r'),
        letter('t'),
        letter('y'),
        letter('u'),
        letter('i'),
        letter('o'),
        letter('p'),
        slot(Key::Backspace, 4),
    ],
    &[
        slot(Key::Symbols, 4),
        letter('a'),
        letter('s'),
        letter('d'),
        letter('f'),
        letter('g'),
        letter('h'),
        letter('j'),
        letter('k'),
        letter('l'),
        letter('\''),
        slot(Key::Send, 4),
    ],
    &[
        slot(Key::CaretLeft, 4),
        letter('z'),
        letter('x'),
        letter('c'),
        letter('v'),
        letter('b'),
        letter('n'),
        letter('m'),
        letter(','),
        letter('.'),
        slot(Key::Space, 4),
        slot(Key::CaretRight, 2),
    ],
];

const SYMBOLS: Rows = [
    &[
        slot(Key::Shift, 4),
        letter('1'),
        letter('2'),
        letter('3'),
        letter('4'),
        letter('5'),
        letter('6'),
        letter('7'),
        letter('8'),
        letter('9'),
        letter('0'),
        slot(Key::Backspace, 4),
    ],
    &[
        slot(Key::Symbols, 4),
        letter('@'),
        letter('#'),
        letter('$'),
        letter('%'),
        letter('&'),
        letter('-'),
        letter('+'),
        letter('('),
        letter(')'),
        letter('"'),
        slot(Key::Send, 4),
    ],
    &[
        slot(Key::CaretLeft, 4),
        letter('!'),
        letter('?'),
        letter(':'),
        letter(';'),
        letter('/'),
        letter('*'),
        letter('='),
        letter('_'),
        letter('~'),
        slot(Key::Space, 4),
        slot(Key::CaretRight, 2),
    ],
];

/// What the caller does with a button after the keyboard has seen it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// The keyboard ignores the button; route it as usual.
    Pass,
    Consumed,
    /// Deliver this draft to the focused application.
    Send(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    #[expect(
        clippy::min_ident_chars,
        reason = "Up matches the other D-pad direction names"
    )]
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    const fn from_button(name: ButtonName) -> Option<Self> {
        match name {
            ButtonName::DpadUp => Some(Self::Up),
            ButtonName::DpadDown => Some(Self::Down),
            ButtonName::DpadLeft => Some(Self::Left),
            ButtonName::DpadRight => Some(Self::Right),
            ButtonName::South
            | ButtonName::East
            | ButtonName::North
            | ButtonName::West
            | ButtonName::LeftBumper
            | ButtonName::RightBumper
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Layer {
    #[default]
    Letters,
    Capitals,
    Symbols {
        capitals: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keyboard {
    open: bool,
    layer: Layer,
    highlight: (usize, usize),
    draft: String,
    caret: usize,
    word: Option<Range<usize>>,
    captured: Vec<ButtonName>,
    repeat: Option<(Direction, Duration)>,
    stick_engaged: bool,
    send_on_start_release: bool,
}

impl Keyboard {
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Opens with an empty draft and the highlight on the first letter.
    pub fn open(&mut self) {
        if self.open {
            return;
        }
        *self = Self {
            open: true,
            highlight: (0, 1),
            captured: core::mem::take(&mut self.captured),
            stick_engaged: self.stick_engaged,
            ..Self::default()
        };
    }

    /// Closes and discards the draft; releases of buttons it consumed stay consumed.
    pub fn close(&mut self) {
        self.open = false;
        self.draft.clear();
        self.caret = 0;
        self.word = None;
        self.repeat = None;
        self.layer = Layer::Letters;
    }

    /// Whether a press of `name` was consumed and its release is still due.
    #[must_use]
    pub fn holds(&self, name: ButtonName) -> bool {
        self.captured.contains(&name)
    }

    /// Keeps the held Start from sending, because it became part of the escape chord.
    pub const fn cancel_send(&mut self) {
        self.send_on_start_release = false;
    }

    /// Forgets held buttons, for a disconnected controller.
    pub fn release_all(&mut self) {
        self.captured.clear();
        self.send_on_start_release = false;
        self.repeat = None;
        self.stick_engaged = false;
    }

    #[must_use]
    pub fn draft(&self) -> &str {
        &self.draft
    }

    /// Byte offset of the caret in the draft.
    #[must_use]
    pub const fn caret(&self) -> usize {
        self.caret
    }

    /// The word the left stick picked, as a byte range of the draft.
    #[must_use]
    pub fn picked_word(&self) -> Option<Range<usize>> {
        self.word.clone()
    }

    /// The highlighted key as (row, index in row).
    #[must_use]
    pub const fn highlight(&self) -> (usize, usize) {
        self.highlight
    }

    #[must_use]
    pub const fn shifted(&self) -> bool {
        matches!(self.layer, Layer::Capitals)
    }

    #[must_use]
    pub const fn symbols(&self) -> bool {
        matches!(self.layer, Layer::Symbols { .. })
    }

    /// The keys of the current layer, top row first.
    #[must_use]
    pub const fn rows(&self) -> &'static Rows {
        if self.symbols() { &SYMBOLS } else { &LETTERS }
    }

    #[must_use]
    pub fn highlighted_key(&self) -> Option<Key> {
        let (row, index) = self.highlight;
        self.rows()
            .get(row)
            .and_then(|keys| keys.get(index))
            .map(|slot| slot.key)
    }

    /// Offers one button. `shoulders_move_caret` is false when LB and RB drive columns.
    pub fn button(&mut self, name: ButtonName, state: ButtonState, shoulders_move_caret: bool) -> Response {
        if name == ButtonName::Guide {
            self.cancel_send();
        }
        if state == ButtonState::Released {
            if name == ButtonName::Start && core::mem::take(&mut self.send_on_start_release) {
                self.captured.retain(|held| *held != name);
                return self.send();
            }
            let Some(index) = self.captured.iter().position(|held| *held == name) else {
                return Response::Pass;
            };
            let _released = self.captured.remove(index);
            if self
                .repeat
                .is_some_and(|_| Direction::from_button(name).is_some())
            {
                self.repeat = None;
            }
            return Response::Consumed;
        }
        if self.captured.contains(&name) {
            return Response::Consumed;
        }
        if !self.open || !captures(name, shoulders_move_caret) {
            return Response::Pass;
        }
        self.captured.push(name);
        self.press(name)
    }

    fn press(&mut self, name: ButtonName) -> Response {
        if let Some(direction) = Direction::from_button(name) {
            self.move_highlight(direction);
            self.repeat = Some((direction, REPEAT_DELAY));
            return Response::Consumed;
        }
        match name {
            ButtonName::South => return self.activate(),
            ButtonName::West => self.backspace(),
            ButtonName::North => self.type_text(" "),
            ButtonName::East => self.close(),
            ButtonName::Start => self.send_on_start_release = true,
            ButtonName::LeftBumper => self.move_caret(false),
            ButtonName::RightBumper => self.move_caret(true),
            ButtonName::LeftStick => self.toggle_symbols(),
            ButtonName::RightStick
            | ButtonName::Select
            | ButtonName::Record
            | ButtonName::Guide
            | ButtonName::DpadUp
            | ButtonName::DpadDown
            | ButtonName::DpadLeft
            | ButtonName::DpadRight => {}
        }
        Response::Consumed
    }

    fn activate(&mut self) -> Response {
        match self.highlighted_key() {
            Some(Key::Char(character)) => {
                let character = if self.shifted() {
                    character.to_uppercase().next().unwrap_or(character)
                } else {
                    character
                };
                self.type_text(character.encode_utf8(&mut [0; 4]));
            }
            Some(Key::Shift) => {
                self.layer = if self.shifted() {
                    Layer::Letters
                } else {
                    Layer::Capitals
                };
            }
            Some(Key::Symbols) => self.toggle_symbols(),
            Some(Key::Backspace) => self.backspace(),
            Some(Key::Send) => return self.send(),
            Some(Key::CaretLeft) => self.move_caret(false),
            Some(Key::CaretRight) => self.move_caret(true),
            Some(Key::Space) => self.type_text(" "),
            None => {}
        }
        Response::Consumed
    }

    fn toggle_symbols(&mut self) {
        self.layer = match self.layer {
            Layer::Letters => Layer::Symbols { capitals: false },
            Layer::Capitals => Layer::Symbols { capitals: true },
            Layer::Symbols { capitals: false } => Layer::Letters,
            Layer::Symbols { capitals: true } => Layer::Capitals,
        };
        self.clamp_highlight();
    }

    fn send(&mut self) -> Response {
        let draft = core::mem::take(&mut self.draft);
        self.close();
        if draft.is_empty() {
            Response::Consumed
        } else {
            Response::Send(draft)
        }
    }

    /// Advances D-pad hold-repeat.
    pub fn tick(&mut self, elapsed: Duration) {
        let Some((direction, mut remaining)) = self.repeat else {
            return;
        };
        let mut elapsed = elapsed.min(Duration::from_secs(1));
        while elapsed >= remaining {
            self.move_highlight(direction);
            elapsed = elapsed.saturating_sub(remaining);
            remaining = REPEAT_INTERVAL;
        }
        self.repeat = Some((direction, remaining.saturating_sub(elapsed)));
    }

    /// Samples the left stick's horizontal deflection; a flick picks the next or previous word.
    pub fn left_stick(&mut self, x: i16) {
        if x.unsigned_abs() < WORD_PICK_RELEASE.unsigned_abs() {
            self.stick_engaged = false;
            return;
        }
        if self.stick_engaged || x.unsigned_abs() < WORD_PICK_DEFLECTION.unsigned_abs() {
            return;
        }
        self.stick_engaged = true;
        if self.open {
            self.pick_word(x > 0);
        }
    }

    /// Inserts dictated text at the caret, or over the picked word, keeping words apart.
    pub fn dictate(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let start = self.word.as_ref().map_or(self.caret, |word| word.start);
        let joined = if self
            .draft
            .get(..start)
            .and_then(|prefix| prefix.chars().next_back())
            .is_some_and(|previous| !previous.is_whitespace())
        {
            format!(" {text}")
        } else {
            text.to_owned()
        };
        self.type_text(&joined);
    }

    fn type_text(&mut self, text: &str) {
        if let Some(word) = self.word.take() {
            self.draft.replace_range(word.clone(), "");
            self.caret = word.start;
        }
        self.draft.insert_str(self.caret, text);
        self.caret = self.caret.saturating_add(text.len());
    }

    fn backspace(&mut self) {
        if let Some(word) = self.word.take() {
            self.draft.replace_range(word.clone(), "");
            self.caret = word.start;
            return;
        }
        if let Some(previous) = self.previous_boundary() {
            self.draft.replace_range(previous..self.caret, "");
            self.caret = previous;
        }
    }

    fn previous_boundary(&self) -> Option<usize> {
        self.draft
            .get(..self.caret)
            .and_then(|prefix| prefix.char_indices().next_back())
            .map(|(index, _)| index)
    }

    fn move_caret(&mut self, right: bool) {
        if let Some(word) = self.word.take() {
            self.caret = if right { word.end } else { word.start };
            return;
        }
        self.caret = if right {
            self.draft
                .get(self.caret..)
                .and_then(|suffix| suffix.chars().next())
                .map_or(self.caret, |next| self.caret.saturating_add(next.len_utf8()))
        } else {
            self.previous_boundary().unwrap_or(0)
        };
    }

    fn words(&self) -> Vec<Range<usize>> {
        let mut words = Vec::new();
        let mut start = None;
        for (index, character) in self.draft.char_indices() {
            match (character.is_whitespace(), start) {
                (false, None) => start = Some(index),
                (true, Some(begin)) => {
                    words.push(begin..index);
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(begin) = start {
            words.push(begin..self.draft.len());
        }
        words
    }

    fn pick_word(&mut self, next: bool) {
        let words = self.words();
        let picked = match (&self.word, next) {
            (Some(current), true) => words.into_iter().find(|word| word.start >= current.end),
            (Some(current), false) => words.into_iter().rev().find(|word| word.end <= current.start),
            (None, true) => words.into_iter().find(|word| word.end > self.caret),
            (None, false) => words.into_iter().rev().find(|word| word.start < self.caret),
        };
        if let Some(word) = picked {
            self.caret = word.end;
            self.word = Some(word);
        }
    }

    fn move_highlight(&mut self, direction: Direction) {
        let rows = self.rows();
        let (row, index) = self.highlight;
        let Some(current) = rows.get(row) else {
            self.highlight = (0, 0);
            return;
        };
        self.highlight = match direction {
            Direction::Left => (
                row,
                index
                    .checked_sub(1)
                    .unwrap_or_else(|| current.len().saturating_sub(1)),
            ),
            Direction::Right => (
                row,
                index.saturating_add(1).checked_rem(current.len()).unwrap_or(0),
            ),
            Direction::Up | Direction::Down => {
                let target = if direction == Direction::Up {
                    row.checked_sub(1).unwrap_or_else(|| rows.len().saturating_sub(1))
                } else {
                    row.saturating_add(1).checked_rem(rows.len()).unwrap_or(0)
                };
                let centre = centre(current, index);
                (target, rows.get(target).map_or(0, |keys| key_at(keys, centre)))
            }
        };
    }

    fn clamp_highlight(&mut self) {
        let (row, index) = self.highlight;
        let length = self.rows().get(row).map_or(0, |keys| keys.len());
        self.highlight = (row, index.min(length.saturating_sub(1)));
    }
}

/// The buttons the open keyboard keeps from the rest of routing.
const fn captures(name: ButtonName, shoulders_move_caret: bool) -> bool {
    match name {
        ButtonName::Guide | ButtonName::Record | ButtonName::Select => false,
        ButtonName::LeftBumper | ButtonName::RightBumper => shoulders_move_caret,
        ButtonName::South
        | ButtonName::East
        | ButtonName::North
        | ButtonName::West
        | ButtonName::Start
        | ButtonName::LeftStick
        | ButtonName::RightStick
        | ButtonName::DpadUp
        | ButtonName::DpadDown
        | ButtonName::DpadLeft
        | ButtonName::DpadRight => true,
    }
}

/// Twice the horizontal centre of a key, in row units.
fn centre(keys: &[Slot], index: usize) -> u32 {
    let start: u32 = keys.iter().take(index).map(|slot| u32::from(slot.width)).sum();
    let width = keys.get(index).map_or(0, |slot| u32::from(slot.width));
    start.saturating_mul(2).saturating_add(width)
}

/// The key covering a doubled horizontal position.
fn key_at(keys: &[Slot], doubled: u32) -> usize {
    let mut end = 0u32;
    for (index, slot) in keys.iter().enumerate() {
        end = end.saturating_add(u32::from(slot.width).saturating_mul(2));
        if doubled < end {
            return index;
        }
    }
    keys.len().saturating_sub(1)
}

/// Whether a transcript asks to open the keyboard.
#[must_use]
pub fn is_open_phrase(text: &str) -> bool {
    text.trim()
        .trim_end_matches(['.', '!', '?'])
        .eq_ignore_ascii_case("keyboard")
}

#[cfg(test)]
mod tests;
