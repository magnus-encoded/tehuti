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
