//! Desktop entry and icon lookup without a display or installed applications.

#[cfg(test)]
mod tests {
    use tehuti::desktop_hints::icon_path;

    #[test]
    fn icon_decoder_rejects_broken_and_oversized_artwork() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("icon.png");
        image::RgbaImage::from_pixel(48, 24, image::Rgba([20, 80, 160, 255])).save(&path)?;
        let decoded = tehuti::desktop_hints::read_icon(&path).ok_or("valid icon rejected")?;
        assert_eq!((decoded.width(), decoded.height()), (48, 24));
        image::RgbaImage::new(1025, 1).save(&path)?;
        assert!(tehuti::desktop_hints::read_icon(&path).is_none());
        std::fs::write(&path, b"broken PNG")?;
        assert!(tehuti::desktop_hints::read_icon(&path).is_none());
        Ok(())
    }

    #[test]
    fn png_lookup_inherits_and_svg_falls_back() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        std::fs::create_dir_all(root.join("custom"))?;
        std::fs::create_dir_all(root.join("hicolor/48x48/apps"))?;
        std::fs::write(
            root.join("custom/index.theme"),
            "[Icon Theme]\nInherits=hicolor\n",
        )?;
        std::fs::write(
            root.join("hicolor/index.theme"),
            "[Icon Theme]\nDirectories=48x48/apps\n",
        )?;
        let png = root.join("hicolor/48x48/apps/reader.png");
        std::fs::write(&png, [])?;
        std::fs::write(root.join("hicolor/48x48/apps/vector.svg"), "<svg/>")?;
        assert_eq!(icon_path("reader", &[root.to_path_buf()], "custom"), Some(png));
        assert_eq!(icon_path("missing", &[root.to_path_buf()], "custom"), None);
        Ok(())
    }

    #[test]
    fn svg_only_theme_icon_is_found_and_rasterised() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        std::fs::create_dir_all(root.join("hicolor/scalable/apps"))?;
        std::fs::write(
            root.join("hicolor/index.theme"),
            "[Icon Theme]\nDirectories=scalable/apps\n",
        )?;
        let svg = root.join("hicolor/scalable/apps/vector.svg");
        std::fs::write(
            &svg,
            "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24'><rect width='24' height='24' fill='#ff0000'/></svg>",
        )?;
        assert_eq!(
            icon_path("vector", &[root.to_path_buf()], "hicolor"),
            Some(svg.clone())
        );
        let icon = tehuti::desktop_hints::read_icon(&svg)
            .ok_or("svg not rasterised")?
            .to_rgba8();
        assert_eq!(icon.dimensions(), (96, 96));
        assert_eq!(icon.get_pixel(48, 48).0, [255, 0, 0, 255]);
        Ok(())
    }

    #[test]
    fn icon_loader_never_blocks_and_bad_svg_yields_none() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let bad = directory.path().join("bad.svg");
        let good = directory.path().join("good.svg");
        std::fs::write(&bad, "<svg><unclosed")?;
        std::fs::write(
            &good,
            "<svg xmlns='http://www.w3.org/2000/svg' width='8' height='8'/>",
        )?;
        let started = std::time::Instant::now();
        let receiver = tehuti::desktop_hints::spawn_icon_loader(vec![Some(bad), None, Some(good)]);
        assert!(started.elapsed() < core::time::Duration::from_millis(100));
        let mut results = std::collections::BTreeMap::new();
        while let Ok((index, icon)) = receiver.recv_timeout(core::time::Duration::from_secs(5)) {
            let _previous = results.insert(index, icon.is_some());
        }
        assert_eq!(
            results.into_iter().collect::<Vec<_>>(),
            vec![(0, false), (2, true)]
        );
        Ok(())
    }
}
