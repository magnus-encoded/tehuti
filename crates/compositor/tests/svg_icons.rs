//! SVG artwork boundary tests.
#[cfg(feature = "svg-icons")]
#[cfg(test)]
mod tests {
    use tehuti::svg_icons::rasterize_svg;

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path fill="red" fill-opacity="0.5" d="M0 0H16V16H0Z"/></svg>"#;

    fn artwork(icon: Option<std::path::PathBuf>) -> Result<Vec<u8>, Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut wallpaper = tehuti::wallpaper::Wallpaper::load(&directory.path().join("missing.png"));
        wallpaper.set_hints(vec![tehuti::desktop_hints::Hint {
            caption: "open reader".into(),
            name: "Reader".into(),
            icon,
        }]);
        let mut pixels = Vec::new();
        wallpaper.buffer((352, 200).into())?.render().draw(|image| {
            pixels = image.to_vec();
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(pixels)
    }

    #[test]
    fn broken_svg_draws_letter_tile() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("broken.svg");
        std::fs::write(&path, b"broken")?;
        assert_eq!(artwork(Some(path.clone()))?, artwork(None)?);
        std::fs::write(&path, SVG)?;
        assert_ne!(artwork(Some(path))?, artwork(None)?);
        Ok(())
    }

    #[test]
    fn valid_svg_renders_straight_alpha_pixels() -> Result<(), Box<dyn core::error::Error>> {
        let image = rasterize_svg(SVG, 32).ok_or("SVG did not render")?;
        assert_eq!(image.dimensions(), (32, 32));
        assert_eq!(image.get_pixel(16, 16).0, [255, 0, 0, 128]);
        Ok(())
    }

    #[test]
    fn broken_svg_returns_none() {
        assert!(rasterize_svg(b"<svg broken", 48).is_none());
    }

    #[test]
    fn oversized_input_and_output_return_none() {
        assert!(rasterize_svg(&vec![b' '; 1_048_577], 48).is_none());
        assert!(rasterize_svg(SVG, 1025).is_none());
        assert!(rasterize_svg(SVG, 0).is_none());
        assert!(
            rasterize_svg(
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="1000000" height="1000000"/>"#,
                48
            )
            .is_none()
        );
    }
}
