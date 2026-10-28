#[cfg(test)]
mod tests {
    use tehuti_core::{
        choice_list::{Candidate, List, Slots},
        pad::PadFamily,
    };

    use smithay::{
        backend::renderer::{element::Element, pixman::PixmanRenderer},
        output::{Mode, Output, PhysicalProperties, Subpixel},
        reexports::wayland_server::Display,
    };
    use tehuti::{
        input::{SyntheticPad, route},
        state::Compositor,
    };
    use tehuti_core::choice_list::{Choice, Outcome};
    use tehuti_protocol::{ButtonName, ButtonState, Event};
    #[test]
    fn panel_leaves_the_surroundings_transparent_and_highlights_selection()
    -> Result<(), Box<dyn core::error::Error>> {
        let list = List::new(Slots {
            title: "Choose".into(),
            candidates: vec![Candidate {
                name: "Example".into(),
                meta: Some("session running".into()),
            }],
            action: "Open".into(),
            ..Slots::default()
        });
        let pixels =
            tehuti::choice_list::raster(&list, PadFamily::Letters, (1920, 1080)).ok_or("missing artwork")?;
        assert_eq!(pixels.get(0..4), Some([0, 0, 0, 0].as_slice()));
        let header_right_edge = (60_usize * 1920 + 1919) * 4;
        assert_eq!(
            pixels.get(header_right_edge..header_right_edge + 4),
            Some([98, 86, 72, 255].as_slice()),
            "the header preserves the thin panel outline",
        );
        let colours: std::collections::HashSet<_> = pixels.as_chunks::<4>().0.iter().copied().collect();
        assert!(colours.len() > 40, "fontdue gives smooth text coverage");
        assert!(
            colours.contains(&[119, 88, 36, 255]),
            "selected row uses the Live tone"
        );
        Ok(())
    }
    #[test]
    fn synthetic_pad_moves_and_confirms_the_cached_overlay() -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        let output = output();
        compositor.open_choice_list(Slots {
            candidates: ["Alpha", "Beta"]
                .into_iter()
                .map(|name| Candidate {
                    name: name.into(),
                    meta: None,
                })
                .collect(),
            action: "Open".into(),
            ..Slots::default()
        });
        let mut renderer = PixmanRenderer::new()?;
        let mut artwork = tehuti::choice_list::Buffer::default();
        let first = artwork
            .element(&mut renderer, &compositor, &output)?
            .ok_or("missing list")?
            .id()
            .clone();
        let repeated = artwork
            .element(&mut renderer, &compositor, &output)?
            .ok_or("missing cached list")?
            .id()
            .clone();
        assert_eq!(first, repeated, "unchanged artwork keeps its element identity");
        let mut pad = SyntheticPad::default();
        tap(&mut compositor, &mut pad, ButtonName::DpadDown)?;
        let moved = artwork
            .element(&mut renderer, &compositor, &output)?
            .ok_or("missing moved list")?
            .id()
            .clone();
        assert_ne!(first, moved, "selection redraws the list");
        tap(&mut compositor, &mut pad, ButtonName::Start)?;
        assert_eq!(
            compositor.choice_list().map(List::outcome),
            Some(Outcome::Picked(Choice::Candidate(1)))
        );
        assert!(artwork.element(&mut renderer, &compositor, &output)?.is_none());
        Ok(())
    }
    fn output() -> Output {
        let output = Output::new(
            "example".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Example".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1280, 720).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        output
    }

    fn tap(compositor: &mut Compositor, pad: &mut SyntheticPad, name: ButtonName) -> std::io::Result<()> {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pad.push(Event::Button { name, state });
        }
        route(compositor, pad)
    }
}
