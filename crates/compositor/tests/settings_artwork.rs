#[cfg(test)]
mod tests {
    use tehuti_core::dispatch::{Page, Settings};

    #[test]
    fn display_modes_draw_a_size_preview_and_refresh_wave() -> Result<(), Box<dyn core::error::Error>> {
        let mut settings = Settings::default();
        settings.open = true;
        settings.page = Page::Display;
        settings.modes = vec!["1920x1080@60".into()];
        let mut buffer = tehuti::settings::buffer(&settings, (1280, 720)).ok_or("missing artwork")?;
        buffer.render().draw(|pixels| {
            for (left, top, width, height) in [(960_usize, 230_usize, 200, 120), (960, 430, 150, 30)] {
                let cyan = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(index, pixel)| {
                        let x = index.checked_rem(1280).unwrap_or_default();
                        let y = index.checked_div(1280).unwrap_or_default();
                        (left..left.saturating_add(width)).contains(&x)
                            && (top..top.saturating_add(height)).contains(&y)
                            && **pixel == [240, 218, 64, 255]
                    })
                    .count();
                assert!(cyan > 100, "the graphical preview must be visible");
            }
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(())
    }
}
