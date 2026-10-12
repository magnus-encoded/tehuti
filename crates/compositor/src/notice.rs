//! A fixed speech notice drawn above client surfaces without taking focus.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::Transform,
};

#[must_use]
pub fn speech_notice_buffer() -> MemoryRenderBuffer {
    let width = 352usize;
    let height = 44usize;
    let mut pixels = vec![0u8; width.saturating_mul(height).saturating_mul(4)];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[32, 32, 32, 255]);
    }
    for (letter, character) in "NO SPEECH MAPPING".chars().enumerate() {
        for (row, bits) in glyph(character).iter().enumerate() {
            for column in 0..5usize {
                if bits
                    & (16u8
                        .checked_shr(u32::try_from(column).unwrap_or_default())
                        .unwrap_or_default())
                    == 0
                {
                    continue;
                }
                for vertical in 0..3usize {
                    for horizontal in 0..3usize {
                        let xpos = 16usize
                            .saturating_add(letter.saturating_mul(20))
                            .saturating_add(column.saturating_mul(3))
                            .saturating_add(horizontal);
                        let ypos = 11usize
                            .saturating_add(row.saturating_mul(3))
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
    MemoryRenderBuffer::from_slice(&pixels, Fourcc::Argb8888, (352, 44), 1, Transform::Normal, None)
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
        _ => [0; 7],
    }
}
