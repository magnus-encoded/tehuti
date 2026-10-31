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
