//! Output bar placement and software imports need no display or DRM device.

#[cfg(test)]
mod tests {
    use smithay::{
        backend::renderer::{element::Element, pixman::PixmanRenderer},
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        utils::Transform,
    };
    use tehuti::bar::Buffer;
    use tehuti_core::{
        State,
        speech::{SpeechInput, SpeechState},
        step,
    };
    use tehuti_protocol::Event;

    fn output(width: i32, height: i32, scale: i32) -> Output {
        let output = Output::new(
            "bar-test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (width, height).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Integer(scale)),
            None,
        );
        output
    }

    #[test]
    fn every_output_gets_a_scaled_bar_at_its_bottom_edge() {
        let mut renderer = PixmanRenderer::new().unwrap();
        for (width, height, scale, top, bar_height) in [(1280, 720, 1, 688, 32), (3840, 2160, 2, 2096, 64)] {
            let output = output(width, height, scale);
            let mut buffer = Buffer::default();
            let state = State::new();
            let (_, position) = buffer.update(&output, state.bar(), "12:34").unwrap();
            assert_eq!(position, (0.0, f64::from(top)));
            let element = buffer
                .element(&mut renderer, &output, state.bar(), "12:34")
                .unwrap()
                .unwrap();
            let rect = element.geometry(f64::from(scale).into());
            assert_eq!(
                (rect.loc.x, rect.loc.y, rect.size.w, rect.size.h),
                (0, top, width, bar_height)
            );
        }
    }

    #[test]
    fn switching_fonts_preserves_chip_edges_height_and_alignment() {
        use tehuti_core::bar::Font;
        for scale in [1, 2] {
            let output = output(1280 * scale, 720 * scale, scale);
            let mut buffer = Buffer::default();
            let mut previous = None;
            for font in [Font::LiberationMono, Font::Bitmap, Font::LiberationMono] {
                let mut bar = State::new().bar().clone();
                bar.font = font;
                let (memory, position) = buffer.update(&output, &bar, "12:34").unwrap();
                assert_eq!(position, (0.0, f64::from(688 * scale)));
                let mut memory = memory.clone();
                let mut pixels = Vec::new();
                memory
                    .render()
                    .draw(|bytes| {
                        pixels.extend_from_slice(bytes);
                        Ok::<_, core::convert::Infallible>(Vec::new())
                    })
                    .unwrap();
                let width = usize::try_from(1280 * scale).unwrap();
                let unit = usize::try_from(scale).unwrap();
                assert_eq!(pixels.len(), width * 32 * unit * 4);
                for row in [0, 31 * unit] {
                    let line = &pixels[row * width * 4..(row + 1) * width * 4];
                    assert_eq!(&line[0..4], &[88, 65, 47, 255]);
                    assert_eq!(&line[200 * unit * 4..200 * unit * 4 + 4], &[30, 24, 20, 255]);
                }
                if let Some(previous) = previous {
                    assert_ne!(pixels, previous, "font changes invalidate the cached bar");
                }
                previous = Some(pixels);
            }
        }
    }

    #[test]
    fn unavailable_font_files_log_and_render_the_bitmap_fallback() -> Result<(), Box<dyn core::error::Error>>
    {
        if std::env::var_os("TEHUTI_TEST_FONT_FALLBACK").is_none() {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("font.ttf");
            for invalid in [false, true] {
                if invalid {
                    std::fs::write(&path, b"invalid font")?;
                }
                let result = std::process::Command::new(std::env::current_exe()?)
                    .args([
                        "--exact",
                        "tests::unavailable_font_files_log_and_render_the_bitmap_fallback",
                        "--nocapture",
                    ])
                    .env("TEHUTI_TEST_FONT_FALLBACK", "1")
                    .env("TEHUTI_BAR_FONT_FILE", &path)
                    .output()?;
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stdout)
                );
                assert!(
                    String::from_utf8_lossy(&result.stderr)
                        .contains("Liberation Mono unavailable; using bitmap bar font")
                );
            }
            return Ok(());
        }
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .try_init()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let output = output(1280, 720, 1);
        let mut buffer = Buffer::default();
        let mut bar = State::new().bar().clone();
        let mut images = Vec::new();
        for font in [
            tehuti_core::bar::Font::LiberationMono,
            tehuti_core::bar::Font::Bitmap,
        ] {
            bar.font = font;
            let (memory, _) = buffer.update(&output, &bar, "12:34").unwrap();
            let mut memory = memory.clone();
            let mut pixels = Vec::new();
            memory.render().draw(|bytes| {
                pixels.extend_from_slice(bytes);
                Ok::<_, core::convert::Infallible>(Vec::new())
            })?;
            images.push(pixels);
        }
        assert_eq!(images[0], images[1]);
        Ok(())
    }

    fn pixels(buffer: &mut Buffer, output: &Output, state: &State) -> Vec<u8> {
        let (memory, _) = buffer.update(output, state.bar(), "12:34").unwrap();
        let mut copy = memory.clone();
        let mut result = Vec::new();
        copy.render()
            .draw(|bytes| {
                result.extend_from_slice(bytes);
                Ok::<_, core::convert::Infallible>(Vec::new())
            })
            .unwrap();
        result
    }

    #[test]
    fn columns_chips_have_flat_edges() {
        use tehuti_protocol::{ButtonName, ButtonState};
        let mut state = State::new();
        for state_of_button in [ButtonState::Pressed, ButtonState::Released] {
            (state, _) = step(
                state,
                &Event::Button {
                    name: ButtonName::Guide,
                    state: state_of_button,
                },
            );
        }
        let bar = state.bar().clone().with_windows(vec![("Notes".into(), true)]);
        let output = output(1280, 720, 1);
        let mut buffer = Buffer::default();
        let (memory, _) = buffer.update(&output, &bar, "12:34").unwrap();
        let mut memory = memory.clone();
        memory
            .render()
            .draw(|bytes| {
                let pixel = |x: usize, y: usize| &bytes[(y * 1280 + x) * 4..(y * 1280 + x) * 4 + 4];
                assert_eq!(pixel(116, 1), pixel(116, 16));
                assert_eq!(pixel(115, 1), &[88, 65, 47, 255]);
                assert_eq!(pixel(116, 16), &[30, 24, 20, 255]);
                Ok::<_, core::convert::Infallible>(Vec::new())
            })
            .unwrap();
    }

    #[test]
    fn overflowing_columns_keep_the_focused_chip_readable_at_either_scale() {
        use tehuti_protocol::{ButtonName, ButtonState};
        let mut state = State::new();
        for state_of_button in [ButtonState::Pressed, ButtonState::Released] {
            (state, _) = step(
                state,
                &Event::Button {
                    name: ButtonName::Guide,
                    state: state_of_button,
                },
            );
        }
        for scale in [1, 2] {
            let output = output(1280 * scale, 720 * scale, scale);
            let mut buffer = Buffer::default();
            for focused in [0, 20, 39, 0] {
                let bar = state
                    .bar()
                    .clone()
                    .with_windows((0..40).map(|index| ("Notes".into(), index == focused)).collect())
                    .with_feedback(tehuti_core::bar::Segment {
                        text: "Ready".into(),
                        tone: tehuti_core::bar::Tone::Context,
                    });
                let (memory, _) = buffer.update(&output, &bar, "12:34").unwrap();
                let mut memory = memory.clone();
                memory
                    .render()
                    .draw(|bytes| {
                        let unit = usize::try_from(scale).unwrap();
                        let row = &bytes[1280 * unit * 4..(1280 + 1176) * unit * 4];
                        let focused_pixels = row
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .filter(|pixel| pixel.as_slice() == [88, 65, 47, 255])
                            .count();
                        assert_eq!(focused_pixels, 116 * unit, "focused window {focused}");
                        let start = row
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .position(|pixel| pixel.as_slice() == [88, 65, 47, 255])
                            .unwrap();
                        assert!(
                            bytes.chunks_exact(1280 * unit * 4).any(|scanline| scanline
                                [start * 4..(start + 116 * unit) * 4]
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .any(|pixel| pixel.as_slice() == [239, 235, 229, 255])),
                            "the focused title has visible text"
                        );
                        Ok::<_, core::convert::Infallible>(Vec::new())
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn context_toned_feedback_does_not_scroll_columns_without_a_focused_window() {
        use tehuti_core::bar::{Segment, Tone};
        use tehuti_protocol::{ButtonName, ButtonState};
        let mut state = State::new();
        for state_of_button in [ButtonState::Pressed, ButtonState::Released] {
            (state, _) = step(
                state,
                &Event::Button {
                    name: ButtonName::Guide,
                    state: state_of_button,
                },
            );
        }
        let bar = state
            .bar()
            .clone()
            .with_windows(vec![("Notes".into(), false), ("Editor".into(), false)])
            .with_feedback(Segment {
                text: "Ready".into(),
                tone: Tone::Context,
            });
        let output = output(304, 720, 1);
        let mut buffer = Buffer::default();
        let (memory, _) = buffer.update(&output, &bar, "12:34").unwrap();
        let mut memory = memory.clone();
        memory
            .render()
            .draw(|bytes| {
                let row = &bytes[304 * 4..(304 + 200) * 4];
                assert!(
                    row.as_chunks::<4>()
                        .0
                        .iter()
                        .all(|pixel| pixel.as_slice() == [55, 43, 34, 255]),
                    "feedback must stay beyond the visible window chips"
                );
                Ok::<_, core::convert::Infallible>(Vec::new())
            })
            .unwrap();
    }

    #[test]
    fn many_long_titles_keep_the_last_window_and_clock_visible() {
        use tehuti_core::bar::Tone;
        use tehuti_protocol::{ButtonName, ButtonState};
        let (state, _) = step(
            State::new(),
            &Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Pressed,
            },
        );
        let (state, _) = step(
            state,
            &Event::Button {
                name: ButtonName::Guide,
                state: ButtonState::Released,
            },
        );
        for scale in [1, 2] {
            let output = output(1280 * scale, 720 * scale, scale);
            let bar = state.bar().clone().with_windows(
                (0..8)
                    .map(|index| ("Long title ".repeat(100), index == 7))
                    .collect(),
            );
            assert_eq!(bar.segments()[7].tone, Tone::Context);
            let mut buffer = Buffer::default();
            let (memory, _) = buffer.update(&output, &bar, "12:34").unwrap();
            let mut memory = memory.clone();
            memory
                .render()
                .draw(|bytes| {
                    let width = usize::try_from(1280 * scale).unwrap();
                    let unit = usize::try_from(scale).unwrap();
                    let row = &bytes[width * 4..width * 8];
                    assert_eq!(&row[1100 * unit * 4..1100 * unit * 4 + 4], &[88, 65, 47, 255]);
                    assert_eq!(&row[1200 * unit * 4..1200 * unit * 4 + 4], &[88, 65, 47, 255]);
                    Ok::<_, core::convert::Infallible>(Vec::new())
                })
                .unwrap();
        }
    }

    #[test]
    fn live_and_result_chips_change_pixels_and_long_text_keeps_the_clock() {
        let output = output(1280, 720, 1);
        let mut buffer = Buffer::default();
        let (mut state, _) = step(
            State::new(),
            &Event::Focus {
                app_id: Some("Claude".into()),
                title: String::new(),
            },
        );
        let context = pixels(&mut buffer, &output, &state);
        let (speech, _) = state.capture(SpeechState::default(), SpeechInput::Available);
        let (speech, _) = state.capture(speech, SpeechInput::Press);
        let listening = pixels(&mut buffer, &output, &state);
        assert_ne!(context, listening, "listening adds a coloured chip");
        let (_, _) = state.capture(
            speech,
            SpeechInput::Interim {
                session: 0,
                text: tehuti_protocol::Text::new("A long live transcript ".repeat(1000)).unwrap(),
            },
        );
        let long_text = pixels(&mut buffer, &output, &state);
        for (short_row, long_row) in listening
            .as_chunks::<5120>()
            .0
            .iter()
            .zip(long_text.as_chunks::<5120>().0.iter())
        {
            assert_eq!(
                short_row.get(4704..),
                long_row.get(4704..),
                "voice text never overwrites the clock chip"
            );
        }
        let (mut state, _) = step(
            state,
            &Event::Speech {
                text: tehuti_protocol::Text::new("unmapped".into()).unwrap(),
            },
        );
        let error = pixels(&mut buffer, &output, &state);
        assert_ne!(listening, error, "errors use a distinct chip");
        state.replace_contexts(vec![
            tehuti_protocol::parse_context("app_id = 'Claude'\nunmatched_speech = 'type'\n[buttons]")
                .unwrap(),
        ]);
        let (state, _) = step(
            state,
            &Event::Speech {
                text: tehuti_protocol::Text::new("hello".into()).unwrap(),
            },
        );
        let result = pixels(&mut buffer, &output, &state);
        assert_ne!(result, error, "successful results use a distinct chip");
        assert_ne!(result, listening, "successful results differ from live text");
    }
}
