#[cfg(test)]
mod archive {
    use core::error::Error;
    use std::path::Path;
    use tehuti::{
        display_mode_store::Store,
        wallpaper::Collection,
        wallpaper_pack::{self, Pack},
    };

    const NAMES: [&str; 13] = [
        "apteribis",
        "bostrychia",
        "cercibis",
        "eudocimus",
        "geronticus",
        "lophotibis",
        "mesembrinibis",
        "nipponia",
        "phimosus",
        "plegadis",
        "pseudibis",
        "theristicus",
        "threskiornis",
    ];

    fn pack(root: &Path, red: u8) -> Result<(), Box<dyn Error>> {
        let folder = root.join("wallpapers");
        std::fs::create_dir_all(&folder)?;
        for name in NAMES {
            image::RgbaImage::from_pixel(2, 2, image::Rgba([red, 0, 0, 255]))
                .save(folder.join(format!("{name}.png")))?;
        }
        wallpaper_pack::build(&folder, &root.join("wallpapers.pak"))?;
        std::fs::remove_dir_all(folder)?;
        Ok(())
    }

    fn red(collection: &mut Collection) -> Result<u8, Box<dyn Error>> {
        let mut result = 0;
        collection.buffer((2, 2).into())?.render().draw(|pixels| {
            result = pixels.first().copied().ok_or("empty buffer")?;
            Ok::<_, Box<dyn Error>>(Vec::new())
        })?;
        Ok(result)
    }

    #[test]
    fn pack_round_trip_contains_every_release_image_without_png_signatures() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        pack(directory.path(), 91)?;
        let path = directory.path().join("wallpapers.pak");
        let archive = Pack::read(&path)?;
        for name in NAMES {
            let image = archive.image(name).ok_or("missing image")?.to_rgba8();
            assert_eq!(image.get_pixel(0, 0).0, [91, 0, 0, 255]);
        }
        assert!(archive.image("../unknown").is_none());
        let bytes = std::fs::read(&path)?;
        assert!(!bytes.windows(8).any(|window| window == b"\x89PNG\r\n\x1a\n"));
        for length in [0, 8, 9, 10, 20, bytes.len().saturating_sub(1)] {
            std::fs::write(&path, bytes.get(..length).ok_or("bad test length")?)?;
            assert!(Pack::read(&path).is_err());
        }
        let mut trailing = bytes;
        trailing.push(0);
        std::fs::write(&path, trailing)?;
        assert!(Pack::read(&path).is_err());
        Ok(())
    }

    #[test]
    fn invalid_index_records_are_rejected() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("wallpapers.pak");
        for records in [
            vec![("unknown", 1)],
            vec![("apteribis", 1), ("apteribis", 1)],
            vec![("apteribis", 0)],
            vec![("apteribis", 64 * 1024 * 1024 + 1)],
        ] {
            let mut bytes = b"TEHUPAK1".to_vec();
            bytes.extend(u16::try_from(records.len())?.to_le_bytes());
            for (name, size) in records {
                bytes.push(u8::try_from(name.len())?);
                bytes.extend(name.as_bytes());
                bytes.extend(u64::try_from(size)?.to_le_bytes());
            }
            std::fs::write(&path, bytes)?;
            assert!(Pack::read(&path).is_err());
        }
        std::fs::write(&path, b"TEHUPAK1\x0e\x00")?;
        assert!(Pack::read(&path).is_err());
        std::fs::write(&path, b"NOTAPAK1\x00\x00")?;
        assert!(Pack::read(&path).is_err());
        Ok(())
    }

    #[test]
    fn roots_precede_format_and_pack_precedes_loose_with_corrupt_pack_fallback() -> Result<(), Box<dyn Error>>
    {
        let directory = tempfile::tempdir()?;
        let user = directory.path().join("user");
        let installed = directory.path().join("installed");
        pack(&user, 40)?;
        pack(&installed, 20)?;
        let roots = [user.clone(), installed.clone()];
        let store = || Store::new(directory.path().join("config/wallpaper"));
        std::fs::create_dir_all(user.join("wallpapers"))?;
        image::RgbaImage::from_pixel(2, 2, image::Rgba([60, 0, 0, 255]))
            .save(user.join("wallpapers/apteribis.png"))?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 0, false))?, 40);
        std::fs::write(user.join("wallpapers.pak"), "corrupt")?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 0, false))?, 60);
        std::fs::remove_file(user.join("wallpapers/apteribis.png"))?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 0, false))?, 20);
        std::fs::write(installed.join("wallpapers.pak"), "corrupt")?;
        assert_eq!(red(&mut Collection::discover(&roots, store(), 0, false))?, 12);
        Ok(())
    }

    #[test]
    fn unlock_and_selection_survive_reopening_without_the_environment_override() -> Result<(), Box<dyn Error>>
    {
        let directory = tempfile::tempdir()?;
        pack(directory.path(), 91)?;
        let roots = [directory.path().to_path_buf()];
        let store = || Store::new(directory.path().join("config/wallpaper"));
        let mut collection = Collection::discover(&roots, store(), 0, false);
        assert_eq!(collection.names(), ["apteribis"]);
        collection.unlock()?;
        assert_eq!(collection.names(), NAMES);
        collection.select(12)?;
        let mut reopened = Collection::discover(&roots, store(), 0, false);
        assert_eq!(reopened.names(), NAMES);
        assert_eq!(red(&mut reopened)?, 91);
        reopened.unlock()?;
        assert_eq!(reopened.names(), NAMES);
        assert_eq!(store().read()?.as_deref(), Some("threskiornis"));
        Ok(())
    }

    #[test]
    fn failed_unlock_does_not_expose_future_wallpapers() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        pack(directory.path(), 91)?;
        let blocked = directory.path().join("blocked");
        std::fs::write(&blocked, "file")?;
        let mut collection = Collection::discover(
            &[directory.path().to_path_buf()],
            Store::new(blocked.join("wallpaper")),
            0,
            false,
        );
        assert!(collection.unlock().is_err());
        assert_eq!(collection.names(), ["apteribis"]);
        Ok(())
    }
}
