//! A fixed speech notice drawn above client surfaces without taking focus.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};

#[must_use]
pub fn speech_notice_buffer() -> MemoryRenderBuffer {
    notice_buffer("NO SPEECH MAPPING")
}

#[must_use]
pub fn notice_buffer(text: &str) -> MemoryRenderBuffer {
    let visible_lines = wrapped_notice_lines(text);
    let columns = visible_lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let width = columns.saturating_mul(12).saturating_add(16).max(64);
    let lines = visible_lines.len().max(1);
    let height = lines.saturating_mul(32);
    let mut pixels = vec![0u8; width.saturating_mul(height).saturating_mul(4)];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[32, 32, 32, 255]);
    }
    for (line, line_text) in visible_lines.iter().enumerate() {
        for (letter, character) in line_text.chars().enumerate() {
            let glyph = glyph(character.to_ascii_uppercase());
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..5usize {
                    if bits
                        & (16u8
                            .checked_shr(u32::try_from(column).unwrap_or_default())
                            .unwrap_or_default())
                        == 0
                    {
                        continue;
                    }
                    for vertical in 0..2usize {
                        for horizontal in 0..2usize {
                            let xpos = 8usize
                                .saturating_add(letter.saturating_mul(12))
                                .saturating_add(column.saturating_mul(2))
                                .saturating_add(horizontal);
                            let ypos = 7usize
                                .saturating_add(line.saturating_mul(30))
                                .saturating_add(row.saturating_mul(2))
                                .saturating_add(vertical);
                            let offset = ypos.saturating_mul(width).saturating_add(xpos).saturating_mul(4);
                            if let Some(pixel) = pixels.get_mut(offset..offset.saturating_add(4)) {
                                pixel.copy_from_slice(&[255, 255, 255, 255]);
                            }
                        }
                    }
                }
            }
        }
    }
    MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Argb8888,
        (
            i32::try_from(width).unwrap_or(512),
            i32::try_from(height).unwrap_or(32),
        ),
        1,
        Transform::Normal,
        None,
    )
}

fn wrapped_notice_lines(text: &str) -> Vec<String> {
    let mut visible_lines = Vec::new();
    for line in text.lines() {
        let mut remaining = line;
        loop {
            let Some((limit, _)) = remaining.char_indices().nth(40) else {
                visible_lines.push(remaining.to_owned());
                break;
            };
            let Some((prefix, suffix)) = remaining.split_at_checked(limit) else {
                break;
            };
            let split = if suffix.starts_with(' ') {
                limit
            } else {
                prefix.rfind(' ').filter(|&space| space > 0).unwrap_or(limit)
            };
            let Some((visible, rest)) = remaining.split_at_checked(split) else {
                break;
            };
            visible_lines.push(visible.to_owned());
            remaining = rest.trim_start_matches(' ');
            if visible_lines.len() == 10 || remaining.is_empty() {
                break;
            }
        }
        if visible_lines.len() == 10 {
            break;
        }
    }
    visible_lines
}

const fn glyph(character: char) -> [u8; 7] {
    match character {
        'N' => [17, 25, 25, 21, 19, 19, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'I' => [31, 4, 4, 4, 4, 4, 31],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        ':' => [0, 4, 4, 0, 4, 4, 0],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '@' => [14, 17, 23, 21, 23, 16, 14],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        '?' => [14, 17, 1, 2, 4, 0, 4],
        _ => [0; 7],
    }
}

#[must_use]
pub fn cursor_buffer() -> MemoryRenderBuffer {
    let mut pixels = vec![0_u8; 24 * 24 * 4];
    for y in 0_usize..24 {
        for x in 0_usize..24 {
            if x <= y / 2 && y < 20 {
                let color = if x == 0 || x == y / 2 || y == 19 { 0 } else { 255 };
                let offset = y.saturating_mul(24).saturating_add(x).saturating_mul(4);
                if let Some(pixel) = pixels.get_mut(offset..offset.saturating_add(4)) {
                    pixel.copy_from_slice(&[color, color, color, 255]);
                }
            }
        }
    }
    MemoryRenderBuffer::from_slice(&pixels, Fourcc::Argb8888, (24, 24), 1, Transform::Normal, None)
}
