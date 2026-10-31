#[cfg(test)]
mod tests {
    use tehuti::notice::notice_buffer;

    fn notice_bytes(text: &str) -> usize {
        let mut buffer = notice_buffer(text);
        let mut bytes = 0;
        buffer
            .render()
            .draw::<_, core::convert::Infallible>(|pixels| {
                bytes = pixels.len();
                Ok(Vec::new())
            })
            .unwrap();
        bytes
    }

    fn notice_pixels(text: &str) -> Vec<u8> {
        let mut buffer = notice_buffer(text);
        let mut result = Vec::new();
        buffer
            .render()
            .draw::<_, core::convert::Infallible>(|pixels| {
                result = pixels.to_vec();
                Ok(Vec::new())
            })
            .unwrap();
        result
    }

    proptest::proptest! {
        #[test]
        fn wrapping_keeps_two_words_whole(
            first in "[A-Z]{20,39}", second in "[A-Z]{20,40}"
        ) {
            proptest::prop_assert_eq!(
                notice_pixels(&format!("{first} {second}")),
                notice_pixels(&format!("{first}\n{second}")),
            );
        }
    }

    #[test]
    fn notice_draws_every_ascii_letter_in_both_cases() {
        for letter in b'A'..=b'Z' {
            let uppercase = notice_pixels(&char::from(letter).to_string());
            assert!(uppercase.as_chunks::<4>().0.contains(&[255; 4]), "{letter}");
            assert_eq!(
                uppercase,
                notice_pixels(&char::from(letter).to_ascii_lowercase().to_string())
            );
        }
    }

    #[test]
    fn notice_wraps_a_word_longer_than_forty_columns_after_a_short_word() {
        assert_eq!(
            notice_pixels(&format!("ON {}", "N".repeat(41))),
            notice_pixels(&format!("ON\n{}\nN", "N".repeat(40))),
        );
    }

    #[test]
    fn notice_keeps_a_forty_column_word_on_one_line() {
        assert_eq!(
            notice_pixels(&format!("{} ON", "N".repeat(40))),
            notice_pixels(&format!("{}\nON", "N".repeat(40))),
        );
    }

    #[test]
    fn notice_draws_x_with_crossed_diagonals() {
        let pixels = notice_pixels("X");
        let rows: Vec<String> = (0..7)
            .map(|row| {
                (0..5)
                    .map(|column| {
                        let offset = ((7 + row * 2) * 64 + 8 + column * 2) * 4;
                        if pixels[offset..offset + 4] == [255; 4] {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect()
            })
            .collect();
        assert_eq!(
            rows,
            ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"]
        );
    }

    #[test]
    fn notice_wraps_at_the_last_space_before_forty_columns() {
        assert_eq!(
            notice_pixels("TEXT MUST WRAP AT A WORD BOUNDARY NEAR THE EDGE"),
            notice_pixels("TEXT MUST WRAP AT A WORD BOUNDARY NEAR\nTHE EDGE"),
        );
    }

    #[test]
    fn short_notice_fits_its_text() {
        assert_eq!(notice_bytes("MOUSE MODE"), 136 * 32 * 4);
    }

    #[test]
    fn multiline_notice_fits_its_longest_visible_line() {
        assert_eq!(notice_bytes("ON\nMOUSE MODE\nOFF"), 136 * 96 * 4);
    }

    #[test]
    fn empty_notice_keeps_a_minimum_size() {
        assert_eq!(notice_bytes(""), 64 * 32 * 4);
    }

    #[test]
    fn long_notice_keeps_the_last_letter_on_a_second_line() {
        let text = format!("{}Z", "N".repeat(40));
        let mut buffer = notice_buffer(&text);
        buffer
            .render()
            .draw::<_, core::convert::Infallible>(|pixels| {
                assert_eq!(pixels.len(), 496 * 64 * 4);
                assert!(pixels[32 * 496 * 4..].as_chunks::<4>().0.contains(&[255; 4]));
                Ok(Vec::new())
            })
            .unwrap();
    }

    #[test]
    fn notice_size_is_bounded_by_visible_text() {
        let text = format!("{}\n{}", "N".repeat(50), "ON\n".repeat(12));
        assert_eq!(notice_bytes(&text), 496 * 320 * 4);
    }
}
