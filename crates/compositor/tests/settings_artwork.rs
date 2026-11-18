#[cfg(test)]
mod tests {
    use tehuti_core::dispatch::{Page, Settings};

    const CYAN: [u8; 4] = [240, 218, 64, 255];
    const FACE: [u8; 4] = [36, 26, 14, 255];

    fn pixels_of(settings: &Settings) -> Result<Vec<[u8; 4]>, Box<dyn core::error::Error>> {
        let mut buffer = tehuti::settings::buffer(settings, "OFF", (1280, 720)).ok_or("missing artwork")?;
        let mut copy = Vec::new();
        buffer.render().draw(|pixels| {
            copy = pixels.as_chunks::<4>().0.to_vec();
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(copy)
    }

    fn pixel_at(pixels: &[[u8; 4]], x: usize, y: usize) -> [u8; 4] {
        pixels
            .get(y.saturating_mul(1280).saturating_add(x))
            .copied()
            .unwrap_or_default()
    }

    #[test]
    fn display_tiles_are_square_rows_with_the_focused_tile_outlined()
    -> Result<(), Box<dyn core::error::Error>> {
        let mut settings = Settings::default();
        settings.open = true;
        settings.page = Page::Display;
        settings.modes = vec!["1920x1080@60".into(), "1920x1080@120".into()];
        let pixels = pixels_of(&settings)?;
        assert_eq!(
            pixel_at(&pixels, 100, 223),
            CYAN,
            "the focused resolution tile has a cyan edge"
        );
        assert_ne!(
            pixel_at(&pixels, 100, 321),
            CYAN,
            "the refresh tile below is not focused"
        );
        Ok(())
    }

    #[test]
    fn font_tiles_show_sample_glyphs() -> Result<(), Box<dyn core::error::Error>> {
        let mut settings = Settings::default();
        settings.open = true;
        settings.page = Page::Display;
        settings.modes = vec!["1920x1080@60".into()];
        settings.display_focus = tehuti_core::tiles::Focus { row: 3, col: 1 };
        let pixels = pixels_of(&settings)?;
        let inked = (440..540)
            .flat_map(|y| (100..260).map(move |x| (x, y)))
            .filter(|(x, y)| pixel_at(&pixels, *x, *y) != FACE)
            .count();
        assert!(inked > 50, "the large sample is drawn inside the first font tile");
        Ok(())
    }

    #[test]
    fn input_tiles_are_equal_squares() -> Result<(), Box<dyn core::error::Error>> {
        let mut settings = Settings::default();
        settings.open = true;
        settings.page = Page::Input;
        settings.input.focus = tehuti_core::tiles::Focus { row: 2, col: 0 };
        let pixels = pixels_of(&settings)?;
        for (x, y) in [(100, 223), (100, 400), (81, 300), (258, 300)] {
            assert_eq!(
                pixel_at(&pixels, x, y),
                CYAN,
                "the focused tile's edge at {x},{y}"
            );
        }
        assert_ne!(
            pixel_at(&pixels, 100, 410),
            CYAN,
            "the tile is as tall as it is wide"
        );
        assert_ne!(
            pixel_at(&pixels, 270, 300),
            CYAN,
            "the tile is as wide as it is tall"
        );
        Ok(())
    }

    fn speech_pixels(speech: &str, colour: [u8; 4]) -> Result<usize, Box<dyn core::error::Error>> {
        let mut settings = Settings::default();
        settings.open = true;
        settings.page = Page::Input;
        let mut buffer = tehuti::settings::buffer(&settings, speech, (1280, 720)).ok_or("missing artwork")?;
        let mut count = 0;
        buffer.render().draw(|pixels| {
            count = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(index, pixel)| {
                    let x = index.checked_rem(1280).unwrap_or_default();
                    let y = index.checked_div(1280).unwrap_or_default();
                    (760..1270).contains(&x) && (84..120).contains(&y) && **pixel == colour
                })
                .count();
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(count)
    }

    #[test]
    fn input_page_shows_the_speech_device_and_marks_the_cpu_in_amber()
    -> Result<(), Box<dyn core::error::Error>> {
        let cyan = [240, 218, 64, 255];
        let amber = [56, 184, 255, 255];
        assert!(
            speech_pixels("CUDA / INT8", cyan)? > 100,
            "a GPU device must be drawn"
        );
        assert!(
            speech_pixels("CPU / INT8", amber)? > 100,
            "a CPU fallback must stand out"
        );
        assert_eq!(speech_pixels("", cyan)?, 0);
        Ok(())
    }

    /// The pixels of the open controller's settings row, with the DEADZONE tile (the fifth) focused.
    fn deadzone_row(deadzone: Option<u16>) -> Result<Vec<[u8; 4]>, Box<dyn core::error::Error>> {
        let mut settings = Settings::default();
        settings.open = true;
        settings.page = Page::Input;
        settings.input.controllers = vec!["Pad".into()];
        if let Some(deadzone) = deadzone {
            let _previous = settings.input.controller_deadzones.insert("Pad".into(), deadzone);
        }
        settings.input.focus = tehuti_core::tiles::Focus { row: 1, col: 4 };
        pixels_of(&settings)
    }

    fn lit(pixels: &[[u8; 4]], area: (usize, usize, usize, usize)) -> usize {
        (area.1..area.1.saturating_add(area.3))
            .flat_map(|y| (area.0..area.0.saturating_add(area.2)).map(move |x| (x, y)))
            .filter(|(x, y)| pixel_at(pixels, *x, *y) == [244, 240, 225, 255])
            .count()
    }

    #[test]
    fn the_deadzone_tile_sits_fifth_in_the_controller_row_with_its_own_value()
    -> Result<(), Box<dyn core::error::Error>> {
        let default = deadzone_row(None)?;
        // Row 1 starts below the first row; the fifth tile is 4 steps of 194 in.
        let tile = (80 + 4 * 194, 222 + 194, 180, 226);
        assert!(lit(&default, tile) > 100, "the tile carries its label and value");
        assert_ne!(
            lit(&default, tile),
            lit(&deadzone_row(Some(250))?, tile),
            "the shown value follows the saved deadzone"
        );
        Ok(())
    }

    #[test]
    fn the_recentre_tile_is_the_last_of_the_row_and_shows_once_measured()
    -> Result<(), Box<dyn core::error::Error>> {
        let drawn = |measured: bool| -> Result<Vec<[u8; 4]>, Box<dyn core::error::Error>> {
            let mut settings = Settings::default();
            settings.open = true;
            settings.page = Page::Input;
            settings.input.controllers = vec!["Pad".into()];
            if measured {
                let _previous = settings
                    .input
                    .controller_recentres
                    .insert("Pad".into(), [-104, 238, 0, 0]);
            }
            settings.input.focus = tehuti_core::tiles::Focus { row: 1, col: 5 };
            pixels_of(&settings)
        };
        // Focused at the sixth tile the row has scrolled, so that tile sits where the fifth did.
        let tile = (80 + 4 * 194, 222 + 194, 180, 226);
        let (open, measured) = (drawn(false)?, drawn(true)?);
        assert!(lit(&open, tile) > 100, "the tile carries its label");
        assert!(
            lit(&measured, tile) > lit(&open, tile),
            "a measured rest adds SET"
        );
        Ok(())
    }
}
