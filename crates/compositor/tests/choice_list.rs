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

    fn launcher(rows: usize) -> tehuti_core::launcher::Launcher {
        let destinations = (0..rows)
            .map(|index| tehuti_core::launcher::Destination {
                name: format!("browser {index}"),
                extra_names: Vec::new(),
                route: "Browser (app)".into(),
                open: false,
                focused: false,
                safety: tehuti_core::launcher::Safety::Launch,
            })
            .collect();
        tehuti_core::launcher::Launcher::new("browser", destinations, None)
    }

    fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
        let start = y.saturating_mul(1920).saturating_add(x).saturating_mul(4);
        pixels
            .get(start..start.saturating_add(4))
            .and_then(|slice| slice.try_into().ok())
            .unwrap_or_default()
    }

    /// Inclusive bounds (left, top, right, bottom) of the pixels matching `wanted` inside `area`.
    fn bounds(
        pixels: &[u8],
        area: (usize, usize, usize, usize),
        wanted: impl Fn([u8; 4]) -> bool,
    ) -> Option<(usize, usize, usize, usize)> {
        let (left, top, right, bottom) = area;
        let mut found = None::<(usize, usize, usize, usize)>;
        for y in top..bottom {
            for x in left..right {
                if wanted(pixel(pixels, x, y)) {
                    found = Some(found.map_or((x, y, x, y), |(low_x, low_y, high_x, high_y)| {
                        (low_x.min(x), low_y.min(y), high_x.max(x), high_y.max(y))
                    }));
                }
            }
        }
        found
    }

    /// Doubled centre of inclusive bounds, so half pixels stay exact.
    fn doubled_centre((left, top, right, bottom): (usize, usize, usize, usize)) -> (usize, usize) {
        (left.saturating_add(right), top.saturating_add(bottom))
    }

    #[test]
    fn pad_face_letters_sit_centred_in_their_discs() -> Result<(), Box<dyn core::error::Error>> {
        use tehuti_core::ticker::{Piece, Rgb};
        let launcher = launcher(3);
        let list = launcher.list();
        let pixels = tehuti::choice_list::raster(list, PadFamily::Letters, (1920, 1080)).ok_or("artwork")?;
        for (piece, _) in [
            list.back_hint(PadFamily::Letters),
            list.selected_hint(PadFamily::Letters).ok_or("hint")?,
        ] {
            let Piece::Glyph {
                colour: Rgb(red, green, blue),
                ..
            } = piece
            else {
                return Err("not a glyph".into());
            };
            let disc = [blue, green, red, 255];
            let (left, top, right, _) =
                bounds(&pixels, (1700, 0, 1920, 1080), |found| found == disc).ok_or("disc missing")?;
            let area = (left, top, right.saturating_add(1), top.saturating_add(28));
            let (disc_x, disc_y) =
                doubled_centre(bounds(&pixels, area, |found| found == disc).ok_or("disc")?);
            let (letter_x, letter_y) = doubled_centre(
                bounds(&pixels, area, |found| {
                    found[3] == 255 && found[0] < 60 && found[1] < 60 && found[2] < 60
                })
                .ok_or("letter missing")?,
            );
            assert!(
                letter_x.abs_diff(disc_x) <= 2 && letter_y.abs_diff(disc_y) <= 2,
                "doubled letter centre {letter_x},{letter_y} vs disc centre {disc_x},{disc_y}"
            );
        }
        Ok(())
    }

    #[test]
    fn launcher_rows_share_the_full_panel_height_above_the_bar() -> Result<(), Box<dyn core::error::Error>> {
        for rows in 1..=5 {
            let launcher = launcher(rows);
            let pixels = tehuti::choice_list::raster(launcher.list(), PadFamily::Letters, (1920, 1080))
                .ok_or("artwork")?;
            let edge = [98, 86, 72, 255];
            let bottom = (0..1080)
                .rev()
                .find(|y| pixel(&pixels, 1280, *y) == edge)
                .ok_or("edge")?;
            assert!(
                (1032..=1048).contains(&bottom),
                "{rows} rows: panel ends at {bottom}, leaving space above the bar"
            );
            let live = [119, 88, 36, 255];
            let selected = (0..1080).filter(|y| pixel(&pixels, 1900, *y) == live).count();
            assert!(
                selected.saturating_mul(rows) >= bottom.saturating_sub(300),
                "{rows} rows: selected row is {selected}px, rows do not fill the panel"
            );
        }
        Ok(())
    }

    #[test]
    fn the_panel_stops_above_a_scaled_bar() -> Result<(), Box<dyn core::error::Error>> {
        let launcher = launcher(2);
        let pixels = tehuti::choice_list::raster_scaled(launcher.list(), PadFamily::Letters, (1920, 1080), 2)
            .ok_or("artwork")?;
        let edge = [98, 86, 72, 255];
        let bottom = (0..1080)
            .rev()
            .find(|y| pixel(&pixels, 1280, *y) == edge)
            .ok_or("edge")?;
        assert!(
            (1000..1016).contains(&bottom),
            "panel ends at {bottom}; the doubled bar starts at 1016"
        );
        Ok(())
    }
}
