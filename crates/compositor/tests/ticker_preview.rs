//! Offscreen previews of the mapping ticker for a symbol and a letter pad.
//!
//! Run by hand with contexts copied from a machine:
//! `TEHUTI_TICKER_CONTEXTS=dir TEHUTI_TICKER_PREVIEW=out cargo test -p tehuti --test ticker_preview -- --ignored`
//! reads `dir/global.toml` and `dir/game.toml` and writes `out/ticker-preview-symbols.png` and
//! `out/ticker-preview-letters.png`, scaled up four times.

#[cfg(test)]
mod preview {
    use tehuti::ticker_strip::{HEIGHT, Strip};
    use tehuti_core::{
        Level,
        ticker::{self, Entry, PadFamily, Rgb},
    };

    const BAR: Rgb = Rgb(20, 24, 30);
    const BAR_HEIGHT: usize = 32;
    const LINE_WIDTH: usize = 560;
    const SCALE: u32 = 4;

    /// Splits entries into lines no wider than `LINE_WIDTH`, so the whole ticker fits on one page.
    fn lines(entries: &[Entry]) -> Vec<Strip> {
        let mut lines = Vec::new();
        let mut line: Vec<Entry> = Vec::new();
        for entry in entries {
            line.push(entry.clone());
            if line.len() > 1 && Strip::paint(&line).width() > LINE_WIDTH {
                let last = line.pop().unwrap();
                lines.push(Strip::paint(&line));
                line = vec![last];
            }
        }
        lines.push(Strip::paint(&line));
        lines
    }

    #[expect(clippy::arithmetic_side_effects, reason = "preview sizes are small constants")]
    fn render(entries: &[Entry]) -> image::RgbImage {
        let lines = lines(entries);
        let width = u32::try_from(LINE_WIDTH + 16).unwrap();
        let height = u32::try_from(BAR_HEIGHT * lines.len()).unwrap();
        let mut picture = image::RgbImage::from_pixel(width, height, image::Rgb([BAR.0, BAR.1, BAR.2]));
        for (index, strip) in lines.iter().enumerate() {
            let top = BAR_HEIGHT * index + (BAR_HEIGHT - HEIGHT) / 2;
            for row in 0..HEIGHT {
                for column in 0..strip.width() {
                    if let Some(Rgb(red, green, blue)) = strip.pixel(column, row) {
                        picture.put_pixel(
                            u32::try_from(column + 8).unwrap(),
                            u32::try_from(top + row).unwrap(),
                            image::Rgb([red, green, blue]),
                        );
                    }
                }
            }
        }
        image::imageops::resize(
            &picture,
            width * SCALE,
            height * SCALE,
            image::imageops::FilterType::Nearest,
        )
    }

    #[test]
    #[ignore = "writes PNG previews; needs TEHUTI_TICKER_CONTEXTS and TEHUTI_TICKER_PREVIEW"]
    fn ticker_previews_for_a_symbol_and_a_letter_pad() {
        let contexts = std::path::PathBuf::from(std::env::var("TEHUTI_TICKER_CONTEXTS").unwrap());
        let out = std::path::PathBuf::from(std::env::var("TEHUTI_TICKER_PREVIEW").unwrap());
        let contexts: Vec<_> = ["global.toml", "game.toml"]
            .iter()
            .map(|name| {
                tehuti_protocol::parse_context(&std::fs::read_to_string(contexts.join(name)).unwrap())
                    .unwrap()
            })
            .collect();
        for (pad, name) in [
            (PadFamily::Symbols, "ticker-preview-symbols.png"),
            (PadFamily::Letters, "ticker-preview-letters.png"),
        ] {
            let entries = ticker::entries(&contexts, Some("org.example.Game"), pad, Level::App);
            assert!(!entries.is_empty(), "the game maps buttons");
            render(&entries).save(out.join(name)).unwrap();
        }
    }
}
