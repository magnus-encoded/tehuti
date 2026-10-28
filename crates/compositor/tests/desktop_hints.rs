//! Desktop launch hints without a display or installed applications.

#[cfg(test)]
mod tests {
    use tehuti::desktop_hints::{entry_metadata, icon_path, layout};

    fn rendered_caption(caption: &str) -> Result<Vec<u8>, Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut wallpaper = tehuti::wallpaper::Wallpaper::load(&directory.path().join("missing.png"));
        wallpaper.set_hints(vec![tehuti::desktop_hints::Hint {
            caption: caption.into(),
            name: "Ærlig".into(),
            icon: None,
        }]);
        let mut pixels = Vec::new();
        wallpaper.buffer((352, 200).into())?.render().draw(|artwork| {
            pixels = artwork.to_vec();
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(pixels)
    }

    #[test]
    fn unicode_captions_draw_distinct_glyphs() -> Result<(), Box<dyn core::error::Error>> {
        let blank = rendered_caption("")?;
        let mut previous = Vec::new();
        for letter in ["å", "ø", "æ"] {
            let pixels = rendered_caption(letter)?;
            assert_ne!(pixels, blank, "caption must draw {letter}");
            assert_ne!(pixels, previous, "letters must have distinct glyphs");
            previous = pixels;
        }
        Ok(())
    }

    proptest::proptest! {
        #[test]
        fn cells_stay_inside_outputs(width in 0_usize..4096, height in 0_usize..2160, count in 0_usize..256) {
            let cells = layout((width, height), count);
            proptest::prop_assert!(cells.len() <= count);
            for (index, &(left, top)) in cells.iter().enumerate() {
                proptest::prop_assert!(left >= 32 && top >= 48);
                proptest::prop_assert!(left.saturating_add(152) <= width);
                proptest::prop_assert!(top.saturating_add(100) <= height);
                for &(other_left, other_top) in cells.iter().skip(index.saturating_add(1)) {
                    proptest::prop_assert!(left.abs_diff(other_left) >= 160 || top.abs_diff(other_top) >= 112);
                }
            }
        }
    }

    #[test]
    fn hints_use_only_the_first_global_context() -> Result<(), String> {
        let parse = |application, phrase| {
            tehuti_protocol::parse_context(&format!(
                "app_id = '{application}'\n[buttons]\n[speech]\n'{phrase}' = {{ launch = 'reader.desktop' }}"
            ))
        };
        let contexts = [
            parse("editor", "Local")?,
            parse("*", "Global")?,
            parse("*", "Other")?,
        ];
        assert_eq!(
            tehuti::desktop_hints::launch_phrases(&contexts),
            vec![("reader.desktop".into(), "Global".into())]
        );
        assert_eq!(
            tehuti::desktop_hints::launch_phrases(core::slice::from_ref(
                contexts.first().ok_or("missing context")?
            )),
            Vec::new()
        );
        Ok(())
    }

    #[test]
    fn changed_phrases_discard_pending_artwork() -> Result<(), Box<dyn core::error::Error>> {
        use smithay::reexports::calloop::{EventLoop, ping::make_ping};
        let directory = tempfile::tempdir()?;
        let mut cache = tehuti::wallpaper::WallpaperCache::new(directory.path().join("missing.png"));
        let mut event_loop = EventLoop::<bool>::try_new()?;
        let (ping, source) = make_ping()?;
        let _token = event_loop
            .handle()
            .insert_source(source, |(), (), ready| *ready = true)?;
        cache.set_redraw_ping(ping);
        assert!(cache.buffer((352, 200).into()).is_none());
        let mut ready = false;
        event_loop.dispatch(core::time::Duration::from_secs(5), &mut ready)?;
        assert!(ready, "worker must finish the old artwork");
        cache.set_launch_phrases(vec![("missing.desktop".into(), "Read".into())]);
        assert!(
            cache.buffer((352, 200).into()).is_none(),
            "old generation must not populate the cache"
        );
        Ok(())
    }

    #[test]
    fn focus_change_keeps_the_old_artwork_until_the_new_is_ready() -> Result<(), Box<dyn core::error::Error>>
    {
        use smithay::reexports::calloop::{EventLoop, ping::make_ping};
        let directory = tempfile::tempdir()?;
        let mut cache = tehuti::wallpaper::WallpaperCache::new(directory.path().join("missing.png"));
        let mut event_loop = EventLoop::<u32>::try_new()?;
        let (ping, source) = make_ping()?;
        let _token = event_loop
            .handle()
            .insert_source(source, |(), (), pings| *pings += 1)?;
        cache.set_redraw_ping(ping);
        assert!(cache.buffer((352, 200).into()).is_none());
        let mut pings = 0;
        event_loop.dispatch(core::time::Duration::from_secs(5), &mut pings)?;
        assert_eq!(pings, 1, "worker must finish the first artwork");
        assert!(cache.buffer((352, 200).into()).is_some());
        cache.set_focus(Some(0));
        assert!(
            cache.buffer((352, 200).into()).is_some(),
            "a focus change must not blank the output while the worker redraws"
        );
        event_loop.dispatch(core::time::Duration::from_secs(5), &mut pings)?;
        assert_eq!(pings, 2, "worker must finish the focused artwork");
        assert!(cache.buffer((352, 200).into()).is_some());
        Ok(())
    }

    #[test]
    fn wallpaper_bakes_png_and_keeps_each_size_cached() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let icon = directory.path().join("reader.png");
        image::RgbaImage::from_pixel(48, 48, image::Rgba([200, 30, 40, 255])).save(&icon)?;
        let mut wallpaper = tehuti::wallpaper::Wallpaper::load(&directory.path().join("missing.png"));
        wallpaper.set_hints(vec![tehuti::desktop_hints::Hint {
            caption: "Åpne øya ærlig".into(),
            name: "Reader".into(),
            icon: Some(icon.clone()),
        }]);
        let mut first = wallpaper.buffer((352, 200).into())?;
        let mut original = Vec::new();
        first.render().draw(|pixels| {
            original = pixels.to_vec();
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        assert_eq!(original.get(67_712..67_716), Some([200, 30, 40, 255].as_slice()));
        std::fs::remove_file(icon)?;
        let mut cached = wallpaper.buffer((352, 200).into())?;
        cached.render().draw(|pixels| {
            assert_eq!(pixels, original, "cached size must not reopen the icon");
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        let mut other = wallpaper.buffer((512, 200).into())?;
        other.render().draw(|pixels| {
            assert_eq!(
                pixels.get(98_432..98_436),
                Some([40, 92, 116, 255].as_slice()),
                "a new size must draw the letter fallback"
            );
            Ok::<_, std::io::Error>(Vec::new())
        })?;
        Ok(())
    }

    #[test]
    fn first_launch_phrase_preserves_order_case_and_unicode() -> Result<(), String> {
        let context = tehuti_protocol::parse_context(
            r"
app_id = '*'
[buttons]
[speech]
'Åpne øya ærlig' = { launch = 'reader.desktop' }
'Another phrase' = { launch = 'reader.desktop' }
'Browse' = { launch = 'browser.desktop' }
'Type' = { text = 'words' }
",
        )?;
        assert_eq!(
            context.launch_phrases(),
            &[
                ("reader.desktop".into(), "Åpne øya ærlig".into()),
                ("browser.desktop".into(), "Browse".into()),
            ]
        );
        Ok(())
    }

    #[test]
    fn metadata_comes_from_main_group() {
        assert_eq!(
            entry_metadata(
                "[Desktop Entry]\nName=Reader\nName[nb]=Leser\nIcon=reader\n[Desktop Action Other]\nName=Other\nIcon=other"
            ),
            Some(("Reader".into(), "reader".into()))
        );
        assert_eq!(entry_metadata("[Desktop Action Other]\nName=Other"), None);
    }

    #[test]
    fn cells_wrap_and_stop_at_output_edges() {
        assert_eq!(
            layout((352, 304), 6),
            vec![(32, 48), (192, 48), (32, 160), (192, 160)]
        );
        assert_eq!(layout((100, 100), 10), Vec::new());
        assert_eq!(layout((0, 0), usize::MAX), Vec::new());
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
        assert_eq!(icon_path("vector", &[root.to_path_buf()], "custom"), None);
        assert_eq!(icon_path("missing", &[root.to_path_buf()], "custom"), None);
        Ok(())
    }
}
