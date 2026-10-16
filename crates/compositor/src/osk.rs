//! Controller selection and native-resolution artwork for the compositor keyboard.

use tehuti_core::dispatch::Sticks;
use tehuti_protocol::{ButtonName, ButtonState};

const LETTERS: [[&str; 20]; 2] = [
    [
        "q", "w", "e", "r", "t", "a", "s", "d", "f", "g", "z", "x", "c", "v", "b", ".", "/", ":", "-", "_",
    ],
    [
        "y", "u", "i", "o", "p", "h", "j", "k", "l", "'", "n", "m", ",", ".", "?", ".com", " ", " ", " ", " ",
    ],
];
const CAPITALS: [[&str; 20]; 2] = [
    [
        "Q", "W", "E", "R", "T", "A", "S", "D", "F", "G", "Z", "X", "C", "V", "B", ".", "/", ":", "-", "_",
    ],
    [
        "Y", "U", "I", "O", "P", "H", "J", "K", "L", "\"", "N", "M", ";", ":", "!", ".com", " ", " ", " ",
        " ",
    ],
];
const SYMBOLS: [[&str; 20]; 2] = [
    [
        "1", "2", "3", "4", "5", "!", "@", "#", "$", "%", "(", ")", "[", "]", "{", ".", "/", ":", "-", "_",
    ],
    [
        "6", "7", "8", "9", "0", "^", "&", "*", "-", "_", "}", "+", "=", "/", "\\", ".com", " ", " ", " ",
        " ",
    ],
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Keyboard {
    pub open: bool,
    selected: [usize; 2],
    layer: usize,
    held: [bool; 8],
    draft: Option<String>,
    cursor: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Text(&'static str),
    Backspace,
    Enter,
    Consumed,
}

impl Keyboard {
    pub fn close(&mut self) {
        self.open = false;
        self.draft = None;
        self.cursor = 0;
    }

    #[must_use]
    pub fn draft(&self) -> Option<&str> {
        self.draft.as_deref()
    }

    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn insert_draft(&mut self, text: &str) {
        let draft = self.draft.get_or_insert_with(String::new);
        draft.insert_str(self.cursor, text);
        self.cursor = self.cursor.saturating_add(text.len());
    }

    pub const fn take_draft(&mut self) -> Option<String> {
        self.cursor = 0;
        self.draft.take()
    }

    pub fn backspace_draft(&mut self) -> bool {
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        let previous = draft
            .get(..self.cursor)
            .and_then(|prefix| prefix.char_indices().next_back())
            .map(|(index, _)| index);
        if let Some(previous) = previous {
            draft.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
        true
    }

    fn move_cursor(&mut self, right: bool) {
        let Some(draft) = &self.draft else {
            return;
        };
        self.cursor = if right {
            draft
                .get(self.cursor..)
                .and_then(|suffix| suffix.chars().next())
                .map_or(self.cursor, |character| {
                    self.cursor.saturating_add(character.len_utf8())
                })
        } else {
            draft
                .get(..self.cursor)
                .and_then(|prefix| prefix.char_indices().next_back())
                .map_or(0, |(index, _)| index)
        };
    }

    pub fn select(&mut self, sticks: Sticks) {
        for (selected, (x, y)) in self.selected.iter_mut().zip([sticks.left, sticks.right]) {
            if x == 0 && y == 0 {
                continue;
            }
            let column = usize::try_from(
                (i32::from(x).clamp(-1000, 1000))
                    .saturating_add(1000)
                    .saturating_mul(5)
                    / 2001,
            )
            .unwrap_or_default();
            let row = usize::try_from(
                (i32::from(y).clamp(-1000, 1000))
                    .saturating_add(1000)
                    .saturating_mul(4)
                    / 2001,
            )
            .unwrap_or_default();
            *selected = row.saturating_mul(5).saturating_add(column);
        }
    }

    #[must_use]
    pub const fn keys(&self) -> &'static [[&'static str; 20]; 2] {
        match self.layer {
            1 => &CAPITALS,
            2 => &SYMBOLS,
            _ => &LETTERS,
        }
    }

    fn key(&self, half: usize) -> &'static str {
        self.selected
            .get(half)
            .and_then(|index| self.keys().get(half)?.get(*index))
            .copied()
            .unwrap_or(" ")
    }

    /// Completion providers (#114) can react to committed text at this effect seam.
    pub fn button(&mut self, name: ButtonName, state: ButtonState) -> Option<Effect> {
        let index = match name {
            ButtonName::LeftBumper => 0,
            ButtonName::RightBumper => 1,
            ButtonName::West => 2,
            ButtonName::North => 3,
            ButtonName::East => 4,
            ButtonName::South => 5,
            ButtonName::DpadLeft if self.draft.is_some() || state == ButtonState::Released => 6,
            ButtonName::DpadRight if self.draft.is_some() || state == ButtonState::Released => 7,
            ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick
            | ButtonName::DpadUp
            | ButtonName::DpadDown
            | ButtonName::DpadLeft
            | ButtonName::DpadRight => return None,
        };
        let held = self.held.get_mut(index)?;
        if state == ButtonState::Released {
            return core::mem::take(held).then_some(Effect::Consumed);
        }
        if !self.open {
            return None;
        }
        if core::mem::replace(held, true) {
            return Some(Effect::Consumed);
        }
        Some(match name {
            ButtonName::LeftBumper => Effect::Text(self.key(0)),
            ButtonName::RightBumper => Effect::Text(self.key(1)),
            ButtonName::West => Effect::Backspace,
            ButtonName::South => Effect::Enter,
            ButtonName::North => {
                self.layer = self.layer.saturating_add(1) % 3;
                Effect::Consumed
            }
            ButtonName::DpadLeft | ButtonName::DpadRight => {
                self.move_cursor(name == ButtonName::DpadRight);
                Effect::Consumed
            }
            ButtonName::East => {
                self.close();
                Effect::Consumed
            }
            ButtonName::Select
            | ButtonName::Record
            | ButtonName::Start
            | ButtonName::Guide
            | ButtonName::LeftStick
            | ButtonName::RightStick
            | ButtonName::DpadUp
            | ButtonName::DpadDown => Effect::Consumed,
        })
    }
}

#[derive(Debug, Default)]
pub struct Artwork {
    cached: Option<(
        Keyboard,
        (i32, i32),
        i32,
        smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    )>,
}

impl Artwork {
    pub fn buffer(
        &mut self,
        keyboard: &Keyboard,
        size: (i32, i32),
        scale: i32,
    ) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> {
        if !keyboard.open {
            self.cached = None;
            return None;
        }
        if self
            .cached
            .as_ref()
            .is_none_or(|(previous, previous_size, previous_scale, _)| {
                previous != keyboard || *previous_size != size || *previous_scale != scale
            })
        {
            self.cached = draw(keyboard, size, scale).map(|buffer| (keyboard.clone(), size, scale, buffer));
        }
        self.cached.as_ref().map(|(_, _, _, buffer)| buffer.clone())
    }
}

/// # Errors
/// Returns the renderer's memory import error.
pub fn element<R>(
    renderer: &mut R,
    compositor: &crate::state::Compositor,
    output: &smithay::output::Output,
) -> Result<Option<smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>>, R::Error>
where
    R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    use smithay::backend::renderer::element::{Kind, memory::MemoryRenderBufferRenderElement};
    if !compositor.osk.open {
        return Ok(None);
    }
    let Some(mode) = output.current_mode() else {
        return Ok(None);
    };
    let scale = output.current_scale().integer_scale().max(1);
    let (y, height) = placement(
        (mode.size.w, mode.size.h),
        scale,
        compositor.osk_cursor_rectangle(scale),
    );
    let Some(buffer) =
        compositor
            .osk_artwork
            .borrow_mut()
            .buffer(&compositor.osk, (mode.size.w, height), scale)
    else {
        return Ok(None);
    };
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, f64::from(y)),
        &buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    )
    .map(Some)
}

/// Physical top and height, keeping the keyboard clear of the field when space permits.
#[must_use]
pub fn placement(size: (i32, i32), scale: i32, cursor: Option<(i32, i32)>) -> (i32, i32) {
    let height = (size.0 / 4).min(size.1 / 2).max(1);
    let bottom = size
        .1
        .saturating_sub(tehuti_core::bar::HEIGHT.saturating_mul(scale.max(1)))
        .saturating_sub(height)
        .max(0);
    let overlaps = cursor.is_some_and(|(top, extent)| {
        top < bottom.saturating_add(height) && top.saturating_add(extent) > bottom
    });
    (if overlaps { 0 } else { bottom }, height)
}

fn draw(
    keyboard: &Keyboard,
    size: (i32, i32),
    scale: i32,
) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer> {
    use smithay::backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer};
    let mut canvas = crate::settings::canvas::Canvas::new(size)?;
    canvas.rectangle(0, 0, 1280, 720, [30, 24, 20, 255]);
    let mut preview = keyboard.draft().unwrap_or("").to_owned();
    preview.insert(keyboard.cursor(), '|');
    let start = preview
        .get(..keyboard.cursor())
        .map_or(0, |prefix| prefix.chars().count())
        .saturating_sub(40);
    let preview: String = preview.chars().skip(start).take(72).collect();
    canvas.text(20, 16, &preview, 4, [239, 235, 229, 255]);
    for (half, (keys, selected_index)) in keyboard.keys().iter().zip(keyboard.selected).enumerate() {
        for (index, key) in keys.iter().enumerate() {
            let x = half
                .saturating_mul(640)
                .saturating_add((index % 5).saturating_mul(124))
                .saturating_add(12);
            let y = (index / 5).saturating_mul(110).saturating_add(100);
            let selected = selected_index == index;
            canvas.rectangle(
                x,
                y,
                114,
                96,
                if selected {
                    [119, 88, 36, 255]
                } else {
                    [88, 65, 47, 255]
                },
            );
            canvas.text(
                x.saturating_add(25),
                y.saturating_add(24),
                if *key == " " { "space" } else { key },
                if *key == " " { 3 } else { 5 },
                [239, 235, 229, 255],
            );
        }
    }
    canvas.text(
        20,
        600,
        "LB/RB: type   X: delete   Y: layer   B: close   A: go",
        4,
        [239, 235, 229, 255],
    );
    canvas.text(20, 660, "D-pad left/right: draft cursor", 3, [239, 235, 229, 255]);
    Some(MemoryRenderBuffer::from_slice(
        &canvas.pixels,
        Fourcc::Argb8888,
        size,
        scale,
        smithay::utils::Transform::Normal,
        None,
    ))
}
