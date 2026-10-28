//! Versioned wallpaper archive with an index and obfuscated PNG blobs.

use std::{
    collections::BTreeMap,
    io::{self, Read as _, Write as _},
    path::Path,
};

const MAGIC: &[u8; 8] = b"TEHUPAK1";
const MASK: u8 = 0xa7;
const MAX_BLOB: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct Pack {
    blobs: BTreeMap<String, Vec<u8>>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid wallpaper pack")
}

impl Pack {
    /// # Errors
    /// Returns an error for an unreadable, truncated or invalid archive.
    pub fn read(path: &Path) -> io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(invalid());
        }
        let mut count = [0; 2];
        file.read_exact(&mut count)?;
        let count = u16::from_le_bytes(count);
        if usize::from(count) > super::wallpaper::CODENAMES.len() {
            return Err(invalid());
        }
        let mut index = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for _ in 0..count {
            let mut length = [0];
            file.read_exact(&mut length)?;
            let mut name = vec![0; usize::from(u8::from_le_bytes(length))];
            file.read_exact(&mut name)?;
            let name = String::from_utf8(name).map_err(|_| invalid())?;
            let mut size = [0; 8];
            file.read_exact(&mut size)?;
            let size = u64::from_le_bytes(size);
            if !super::wallpaper::CODENAMES.contains(&name.as_str())
                || !names.insert(name.clone())
                || size == 0
                || size > MAX_BLOB
            {
                return Err(invalid());
            }
            index.push((name, size));
        }
        let mut blobs = BTreeMap::new();
        for (name, size) in index {
            let mut blob = vec![0; usize::try_from(size).map_err(|_| invalid())?];
            file.read_exact(&mut blob)?;
            for byte in &mut blob {
                *byte ^= MASK;
            }
            let _previous = blobs.insert(name, blob);
        }
        if file.read(&mut [0])? != 0 {
            return Err(invalid());
        }
        Ok(Self { blobs })
    }

    #[must_use]
    pub fn image(&self, name: &str) -> Option<image::DynamicImage> {
        image::load_from_memory_with_format(self.blobs.get(name)?, image::ImageFormat::Png).ok()
    }
}

/// Builds an archive containing all release codenames from `<codename>.png` files.
/// # Errors
/// Returns an error for missing or invalid PNGs, or an unwritable destination.
pub fn build(folder: &Path, destination: &Path) -> io::Result<()> {
    let mut blobs = Vec::new();
    for name in super::wallpaper::CODENAMES {
        let blob = std::fs::read(folder.join(format!("{name}.png")))?;
        if u64::try_from(blob.len()).map_err(|_| invalid())? > MAX_BLOB {
            return Err(invalid());
        }
        let _image =
            image::load_from_memory_with_format(&blob, image::ImageFormat::Png).map_err(io::Error::other)?;
        blobs.push((*name, blob));
    }
    let temporary = destination.with_extension("pak.tmp");
    let mut file = std::io::BufWriter::new(std::fs::File::create(&temporary)?);
    file.write_all(MAGIC)?;
    file.write_all(&u16::try_from(blobs.len()).map_err(|_| invalid())?.to_le_bytes())?;
    for (name, blob) in &blobs {
        file.write_all(&[u8::try_from(name.len()).map_err(|_| invalid())?])?;
        file.write_all(name.as_bytes())?;
        file.write_all(&u64::try_from(blob.len()).map_err(|_| invalid())?.to_le_bytes())?;
    }
    for (_, mut blob) in blobs {
        for byte in &mut blob {
            *byte ^= MASK;
        }
        file.write_all(&blob)?;
    }
    file.flush()?;
    std::fs::rename(temporary, destination)
}
