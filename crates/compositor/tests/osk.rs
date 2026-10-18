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
        for name in [
            ButtonName::Guide,
            ButtonName::GuideTap,
            ButtonName::Record,
            ButtonName::KeyboardPtt,
        ] {
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
    fn unassigned_buttons_are_consumed_and_their_releases_stay_captured_after_close() {
        for name in [
            ButtonName::Select,
            ButtonName::Start,
            ButtonName::LeftStick,
            ButtonName::RightStick,
            ButtonName::DpadUp,
            ButtonName::DpadDown,
            ButtonName::DpadLeft,
            ButtonName::DpadRight,
            ButtonName::Paddle1,
            ButtonName::Paddle2,
            ButtonName::Paddle3,
        ] {
            let mut keyboard = Keyboard::default();
            keyboard.open = true;
            assert_eq!(
                keyboard.button(name, ButtonState::Pressed),
                Some(Effect::Consumed)
            );
            keyboard.close();
            assert_eq!(
                keyboard.button(name, ButtonState::Released),
                Some(Effect::Consumed)
            );
            assert_eq!(keyboard.button(name, ButtonState::Pressed), None);
        }
    }

    #[test]
    fn paddle_four_hides_and_restores_the_keyboard_and_draft() {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        keyboard.insert_draft("hello");
        assert_eq!(press(&mut keyboard, ButtonName::Paddle4), Some(Effect::Consumed));
        assert!(!keyboard.open);
        assert_eq!(press(&mut keyboard, ButtonName::Paddle4), Some(Effect::Consumed));
        assert!(keyboard.open);
        assert_eq!(keyboard.draft(), Some("hello"));
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
    fn completion_strip_and_each_key_row_have_bar_height() -> Result<(), Box<dyn core::error::Error>> {
        let mut keyboard = Keyboard::default();
        keyboard.open = true;
        let mut artwork = tehuti::osk::Artwork::default();
        for (width, height, scale) in [(640, 160, 1), (1920, 160, 1), (3840, 320, 2)] {
            let mut buffer = artwork
                .buffer(&keyboard, (width, height), scale)
                .ok_or("no keyboard artwork")?;
            buffer.render().draw(|pixels| {
                assert_eq!(pixels.len(), usize::try_from(width * height * 4)?);
                let pixel = |x: i32, y: i32| -> Result<&[u8], Box<dyn core::error::Error>> {
                    let offset = usize::try_from((y * width + x) * 4)?;
                    Ok(&pixels[offset..offset + 4])
                };
                let row_height = 32 * scale;
                assert_eq!(pixel(2 * scale, row_height - scale)?, &[30, 24, 20, 255]);
                for row in 1..5 {
                    assert_eq!(pixel(2 * scale, row * row_height)?, &[30, 24, 20, 255]);
                    assert_eq!(
                        pixel(2 * scale, row * row_height + scale)?,
                        if row == 1 {
                            &[119, 88, 36, 255]
                        } else {
                            &[88, 65, 47, 255]
                        }
                    );
                }
                Ok::<_, Box<dyn core::error::Error>>(Vec::new())
            })?;
        }
        keyboard.open = false;
        assert!(artwork.buffer(&keyboard, (1920, 160), 1).is_none());
        Ok(())
    }
    #[test]
    fn keyboard_never_overlaps_the_bar() {
        for scale in [1, 2] {
            for output_height in [32, 64, 100, 1080, 2160] {
                for cursor in [None, Some((900, 24))] {
                    let (top, height) = tehuti::osk::placement((3840, output_height), scale, cursor);
                    assert!(top + height <= (output_height - 32 * scale).max(0));
                }
            }
        }
    }

    #[test]
    fn overlay_sits_above_the_bar_and_avoids_bottom_fields() {
        assert_eq!(tehuti::osk::placement((1920, 1080), 1, None), (888, 160));
        assert_eq!(tehuti::osk::placement((1920, 1080), 1, Some((950, 24))), (0, 160));
        assert_eq!(
            tehuti::osk::placement((1920, 1080), 1, Some((100, 24))),
            (888, 160)
        );
        assert_eq!(tehuti::osk::placement((3840, 2160), 2, None), (1776, 320));
    }
}
