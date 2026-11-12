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
        assert_eq!(
            pixel(&pixels, 1919, 60),
            [98, 86, 72, 255],
            "the header preserves the thin panel outline",
        );
        let colours: std::collections::HashSet<_> = pixels.as_chunks::<4>().0.iter().copied().collect();
        assert!(colours.len() > 40, "fontdue gives smooth text coverage");
        assert!(
            colours.contains(&[151, 117, 57, 255]),
            "missing icons fall back to the initial tile"
        );
        assert!(
            colours.contains(&[119, 88, 36, 255]),
            "selected row uses the Live tone"
        );
        Ok(())
    }
    #[test]
    fn long_entry_lines_render_as_padded_ellipses() -> Result<(), Box<dyn core::error::Error>> {
        let artwork = |name: String, meta: String| {
            tehuti::choice_list::raster(
                &List::ordered(Slots {
                    candidates: vec![Candidate {
                        name,
                        meta: Some(meta),
                    }],
                    action: "Open".into(),
                    ..Slots::default()
                }),
                PadFamily::Letters,
                (1920, 1080),
            )
            .ok_or("artwork")
        };
        let long = artwork("W".repeat(200), "W".repeat(200))?;
        let truncated = artwork(format!("{}…", "W".repeat(15)), format!("{}…", "W".repeat(31)))?;
        assert!(
            long == truncated,
            "each line reserves room for a complete ellipsis"
        );
        for y in 112..168 {
            for x in 1876..1892 {
                assert_eq!(
                    pixel(&long, x, y),
                    [119, 88, 36, 255],
                    "right padding stays clear"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn unicode_and_line_breaks_stay_inside_the_entry() -> Result<(), Box<dyn core::error::Error>> {
        for size in [(1920, 1080), (1280, 720), (2560, 1440)] {
            let render = |suffix: &str| {
                tehuti::choice_list::raster(
                    &List::ordered(Slots {
                        candidates: vec![Candidate {
                            name: format!("{}{}", "Æ界e\u{301}\n".repeat(100), suffix),
                            meta: Some(format!("{}{}", "日本語\t".repeat(100), suffix)),
                        }],
                        ..Slots::default()
                    }),
                    PadFamily::Letters,
                    size,
                )
                .ok_or("artwork")
            };
            assert!(
                render("one")? == render("two")?,
                "overflow cannot alter pixels at {size:?}"
            );
        }
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
        let top = (0_usize..1080)
            .find(|&row| {
                let start = row.saturating_mul(1920).saturating_add(640).saturating_mul(4);
                pixels.get(start..start.saturating_add(4)) == Some([98, 86, 72, 255].as_slice())
            })
            .unwrap_or_default();
        let x = x.saturating_sub(640);
        let y = y.saturating_add(top);
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
    fn launcher_sizes_to_content_and_centres_on_every_aspect_ratio() -> Result<(), Box<dyn core::error::Error>>
    {
        for size in [(3840, 1600), (1920, 1080), (800, 1200), (640, 360)] {
            let mut previous = 0;
            for rows in 0..=5 {
                let launcher = launcher(rows);
                let pixels = tehuti::choice_list::raster(launcher.list(), PadFamily::Letters, size)
                    .ok_or("artwork")?;
                let width = usize::try_from(size.0)?;
                let edges: Vec<_> = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    // Scaled edges are antialiased, so find the opaque panel rather than its edge colour.
                    .filter(|(_, pixel)| pixel[3] >= 200)
                    .map(|(index, _)| (index % width, index / width))
                    .collect();
                let left = edges.iter().map(|edge| edge.0).min().ok_or("left")?;
                let right = edges.iter().map(|edge| edge.0).max().ok_or("right")?;
                let top = edges.iter().map(|edge| edge.1).min().ok_or("top")?;
                let bottom = edges.iter().map(|edge| edge.1).max().ok_or("bottom")?;
                assert!(
                    left.abs_diff(width - right - 1) <= 1,
                    "horizontal centre at {size:?}"
                );
                let available = usize::try_from(size.1 - tehuti_core::bar::HEIGHT)?;
                assert!(
                    top.abs_diff(available - bottom - 1) <= 1,
                    "vertical centre at {size:?}"
                );
                let height = bottom - top + 1;
                assert!(height <= available * 4 / 5, "height cap at {size:?}");
                assert!(height >= previous, "more rows cannot shrink the panel");
                previous = height;
            }
        }
        Ok(())
    }

    /// The opaque panel's last row; scaled edges are antialiased, so opacity marks the panel.
    fn opaque_bottom(pixels: &[u8]) -> Option<usize> {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .rposition(|pixel| pixel[3] >= 200)
            .map(|index| index / 1920)
    }

    #[test]
    fn launcher_rows_keep_their_natural_height_above_the_bar() -> Result<(), Box<dyn core::error::Error>> {
        let bar = usize::try_from(tehuti_core::bar::HEIGHT)?;
        for rows in 1..=5 {
            let launcher = launcher(rows);
            let pixels = tehuti::choice_list::raster(launcher.list(), PadFamily::Letters, (1920, 1080))
                .ok_or("artwork")?;
            let bottom = opaque_bottom(&pixels).ok_or("panel")?;
            // Natural height is 160 + 72 per row, centred in the 1048 rows above the bar.
            let natural = 160_usize.saturating_add(rows.saturating_mul(72));
            let expected = (1080_usize.saturating_sub(bar).saturating_sub(natural) / 2)
                .saturating_add(natural)
                .saturating_sub(1);
            assert_eq!(bottom, expected, "{rows} rows: panel bottom row");
            assert!(
                bottom < 1080_usize.saturating_sub(bar),
                "{rows} rows: above the bar"
            );
            let live = [119, 88, 36, 255];
            let selected = (0..1080).filter(|y| pixel(&pixels, 1700, *y) == live).count();
            assert!(
                selected == 72,
                "{rows} rows: selected row is {selected}px, row must keep its natural height"
            );
        }
        Ok(())
    }

    #[test]
    fn the_panel_stops_above_a_scaled_bar() -> Result<(), Box<dyn core::error::Error>> {
        let launcher = launcher(2);
        let pixels = tehuti::choice_list::raster_scaled(launcher.list(), PadFamily::Letters, (1920, 1080), 2)
            .ok_or("artwork")?;
        let bottom = opaque_bottom(&pixels).ok_or("panel")?;
        // 2 rows: natural 304, panel 1280 wide (640 at scale 2) and 608 tall, centred in the
        // 1920 x (1080 - 2 * 32) area above the doubled bar: left 320, top 204.
        assert_eq!(bottom, 811, "panel bottom row; the doubled bar starts at 1016");
        let covered = |x: &usize| {
            (0..1080_usize).any(|y| {
                pixels
                    .get(
                        y.saturating_mul(1920)
                            .saturating_add(*x)
                            .saturating_mul(4)
                            .saturating_add(3),
                    )
                    .is_some_and(|alpha| *alpha >= 200)
            })
        };
        assert_eq!((0..1920_usize).find(covered), Some(320), "panel left column");
        assert_eq!(
            (0..1920_usize).rev().find(covered),
            Some(1599),
            "panel right column mirrors the left"
        );
        Ok(())
    }

    #[test]
    fn the_empty_launcher_keeps_every_line_inside_its_panel() -> Result<(), Box<dyn core::error::Error>> {
        let launcher = tehuti_core::launcher::Launcher::invitation(None);
        let pixels = tehuti::choice_list::raster(launcher.list(), PadFamily::Letters, (1920, 1080))
            .ok_or("artwork")?;
        let bottom = opaque_bottom(&pixels).ok_or("panel")?;
        let pixel_at = |x: usize, y: usize| {
            let start = y.saturating_mul(1920).saturating_add(x).saturating_mul(4);
            pixels
                .get(start..start.saturating_add(4))
                .unwrap_or_default()
                .to_vec()
        };
        let left = (0..1920_usize)
            .find(|&x| pixel_at(x, bottom.saturating_sub(40))[3] >= 200)
            .ok_or("panel left")?;
        let background = pixel_at(left.saturating_add(4), bottom.saturating_sub(1));
        // The last 16 rows above the bottom border are padding; text there means the panel cut it off.
        for y in bottom.saturating_sub(16)..bottom {
            for x in left.saturating_add(2)..left.saturating_add(636) {
                assert_eq!(
                    pixel_at(x, y),
                    background,
                    "text touches the bottom edge at {x},{y}"
                );
            }
        }
        Ok(())
    }
}
