//! Controller keyboard selection through its public input seam.

#[cfg(test)]
mod tests {
    use tehuti::osk::{Effect, Keyboard};
    use tehuti_core::dispatch::Sticks;
    use tehuti_protocol::{ButtonName, ButtonState};

    fn press(keyboard: &mut Keyboard, button: ButtonName) -> Option<Effect> {
        let effect = keyboard.button(button, ButtonState::Pressed);
        assert_eq!(
            keyboard.button(button, ButtonState::Released),
            Some(Effect::Consumed)
        );
        effect
    }

    #[test]
    fn both_sticks_select_their_qwerty_half_and_neutral_preserves_selection() {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        for (sticks, left, right) in [
            (
                Sticks {
                    left: (-1000, -1000),
                    right: (1000, -1000),
                },
                "q",
                "p",
            ),
            (
                Sticks {
                    left: (0, -200),
                    right: (-1000, -200),
                },
                "d",
                "h",
            ),
            (
                Sticks {
                    left: (1000, 400),
                    right: (1000, 400),
                },
                "b",
                "?",
            ),
            (
                Sticks {
                    left: (-1000, 1000),
                    right: (1000, 1000),
                },
                ".",
                " ",
            ),
        ] {
            keyboard.select(sticks);
            keyboard.select(Sticks::default());
            assert_eq!(
                press(&mut keyboard, ButtonName::LeftBumper),
                Some(Effect::Text(left))
            );
            assert_eq!(
                press(&mut keyboard, ButtonName::RightBumper),
                Some(Effect::Text(right))
            );
        }
    }

    #[test]
    fn north_cycles_capitals_symbols_and_letters_without_changing_selection() {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        for (left, right) in [("Q", "Y"), ("1", "6"), ("q", "y")] {
            assert_eq!(press(&mut keyboard, ButtonName::North), Some(Effect::Consumed));
            assert_eq!(
                press(&mut keyboard, ButtonName::LeftBumper),
                Some(Effect::Text(left))
            );
            assert_eq!(
                press(&mut keyboard, ButtonName::RightBumper),
                Some(Effect::Text(right))
            );
        }
    }

    #[test]
    fn backspace_enter_close_and_reserved_buttons_keep_their_meanings() {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        assert_eq!(press(&mut keyboard, ButtonName::West), Some(Effect::Backspace));
        assert_eq!(press(&mut keyboard, ButtonName::South), Some(Effect::Enter));
        for name in [ButtonName::Guide, ButtonName::Record, ButtonName::DpadUp] {
            assert_eq!(keyboard.button(name, ButtonState::Pressed), None);
        }
        assert_eq!(press(&mut keyboard, ButtonName::East), Some(Effect::Consumed));
        assert!(!keyboard.open);
        assert_eq!(
            keyboard.button(ButtonName::LeftBumper, ButtonState::Pressed),
            None
        );
    }

    #[test]
    fn holding_a_bumper_commits_only_once_until_release() {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        assert_eq!(
            keyboard.button(ButtonName::LeftBumper, ButtonState::Pressed),
            Some(Effect::Text("q"))
        );
        assert_eq!(
            keyboard.button(ButtonName::LeftBumper, ButtonState::Pressed),
            Some(Effect::Consumed)
        );
        assert_eq!(
            keyboard.button(ButtonName::LeftBumper, ButtonState::Released),
            Some(Effect::Consumed)
        );
        assert_eq!(
            press(&mut keyboard, ButtonName::LeftBumper),
            Some(Effect::Text("q"))
        );
    }
    #[test]
    fn draft_cursor_edits_unicode_and_close_discards_uncommitted_text() {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        keyboard.insert_draft("a🦀é");
        assert_eq!(press(&mut keyboard, ButtonName::DpadLeft), Some(Effect::Consumed));
        assert!(keyboard.backspace_draft());
        assert_eq!(keyboard.draft(), Some("aé"));
        keyboard.insert_draft("/");
        assert_eq!(keyboard.draft(), Some("a/é"));
        for _ in 0..8 {
            let _effect = press(&mut keyboard, ButtonName::DpadRight);
        }
        keyboard.insert_draft(".com");
        assert_eq!(keyboard.take_draft().as_deref(), Some("a/é.com"));
        assert!(!keyboard.backspace_draft());
        keyboard.insert_draft("discard");
        keyboard.close();
        assert_eq!(keyboard.draft(), None);
        assert_eq!(keyboard.cursor(), 0);
    }

    #[test]
    fn every_layer_has_the_url_row() {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        for _ in 0..3 {
            assert_eq!(&keyboard.keys()[0][15..], &[".", "/", ":", "-", "_"]);
            assert_eq!(keyboard.keys()[1][15], ".com");
            let _effect = press(&mut keyboard, ButtonName::North);
        }
    }

    #[test]
    fn artwork_rasterizes_vector_edges_at_each_native_mode() -> Result<(), Box<dyn core::error::Error>> {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        let mut artwork = tehuti::osk::Artwork::default();
        for (width, height, scale) in [(640, 160, 1), (1920, 480, 1), (3840, 960, 2)] {
            let mut buffer = artwork
                .buffer(&keyboard, (width, height), scale)
                .ok_or("no keyboard artwork")?;
            buffer.render().draw(|pixels| {
                assert_eq!(pixels.len(), usize::try_from(width * height * 4)?);
                let colors: std::collections::HashSet<_> =
                    pixels.as_chunks::<4>().0.iter().copied().collect();
                assert!(
                    colors.len() > 30,
                    "vector glyph edges have fractional coverage at {width}x{height}"
                );
                Ok::<_, Box<dyn core::error::Error>>(Vec::new())
            })?;
        }
        keyboard.open = false;
        assert!(artwork.buffer(&keyboard, (1920, 480), 1).is_none());
        Ok(())
    }
    #[test]
    fn overlay_sits_above_the_bar_and_avoids_bottom_fields() {
        assert_eq!(tehuti::osk::placement((1920, 1080), 1, None), (568, 480));
        assert_eq!(tehuti::osk::placement((1920, 1080), 1, Some((800, 24))), (0, 480));
        assert_eq!(
            tehuti::osk::placement((1920, 1080), 1, Some((100, 24))),
            (568, 480)
        );
        assert_eq!(tehuti::osk::placement((3840, 2160), 2, None), (1136, 960));
    }
}
