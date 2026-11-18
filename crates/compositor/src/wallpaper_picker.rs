//! Folder rescans and physical wallpaper thumbnails prepared away from drawing.

use super::wallpaper::{Collection, Source};
use smithay::reexports::calloop::ping::Ping;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::mpsc,
};

type Key = (u64, usize, u32, u32);

const MEMORY_CAP: usize = 64 * 1024 * 1024;
const DISK_CAP: u64 = 128 * 1024 * 1024;
const DISK_MAX_AGE: core::time::Duration = core::time::Duration::from_secs(30 * 24 * 3600);

/// Decoded thumbnails kept under a byte cap; the least recently used leave first.
#[derive(Debug)]
struct Memory {
    entries: HashMap<Key, (image::RgbaImage, u64)>,
    bytes: usize,
    clock: u64,
    cap: usize,
}

impl Memory {
    fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
            cap,
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn get(&mut self, key: &Key) -> Option<image::RgbaImage> {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        let (image, used) = self.entries.get_mut(key)?;
        *used = clock;
        Some(image.clone())
    }

    fn insert(&mut self, key: Key, image: image::RgbaImage) {
        self.clock = self.clock.wrapping_add(1);
        let size = image.as_raw().len();
        if let Some((old, _)) = self.entries.insert(key, (image, self.clock)) {
            self.bytes = self.bytes.saturating_sub(old.as_raw().len());
        }
        self.bytes = self.bytes.saturating_add(size);
        while self.bytes > self.cap && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(other, _)| **other != key)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(other, _)| *other)
            else {
                break;
            };
            if let Some((evicted, _)) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(evicted.as_raw().len());
            }
        }
    }
}

/// The thumbnail cache folder on disk.
pub(super) fn cache_directory() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|base| base.join("tehuti/wallpaper-thumbnails"))
}

/// Removes leftovers and files unused for `max_age`, then the oldest files until `cap` bytes remain.
fn prune_disk(directory: &Path, cap: u64, max_age: core::time::Duration) {
    let Ok(files) = std::fs::read_dir(directory) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut kept = Vec::new();
    for file in files.filter_map(Result::ok) {
        let path = file.path();
        let Ok(meta) = file.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        let stale = now.duration_since(modified).is_ok_and(|age| age > max_age);
        if path.extension().is_some_and(|ext| ext == "tmp") || stale {
            let _removed = std::fs::remove_file(&path);
        } else {
            kept.push((modified, meta.len(), path));
        }
    }
    kept.sort();
    let mut total: u64 = kept.iter().map(|(_, size, _)| *size).sum();
    for (_, size, path) in kept {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

#[derive(Debug)]
enum Job {
    Refresh,
    Thumbnail {
        key: Key,
        source: Source,
        identity: String,
        cache: PathBuf,
    },
}

#[derive(Debug)]
enum Reply {
    Collection(Box<Collection>),
    Thumbnail(Key, Option<image::RgbaImage>),
}

#[derive(Debug)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "the picker worker is shared by wallpaper collection and rendering"
)]
pub(super) struct Picker {
    requests: mpsc::SyncSender<(Job, Option<Ping>)>,
    replies: mpsc::Receiver<Reply>,
    ready: Memory,
    pending: HashSet<Key>,
    failed: HashSet<Key>,
    pub(super) revision: u64,
    epoch: u64,
    refreshing: bool,
    refresh_requested: bool,
    found: Option<Collection>,
}

impl Picker {
    pub(super) fn new() -> Self {
        let (requests, jobs) = mpsc::sync_channel::<(Job, Option<Ping>)>(8);
        let (results, replies) = mpsc::channel();
        if let Err(error) = std::thread::Builder::new()
            .name("wallpaper-picker".into())
            .spawn(move || {
                while let Ok((job, ping)) = jobs.recv() {
                    let reply = match job {
                        Job::Refresh => {
                            if let Some(directory) = cache_directory() {
                                prune_disk(&directory, DISK_CAP, DISK_MAX_AGE);
                            }
                            Reply::Collection(Box::new(Collection::from_env()))
                        }
                        Job::Thumbnail {
                            key,
                            source,
                            identity,
                            cache,
                        } => Reply::Thumbnail(key, thumbnail(&source, &identity, (key.2, key.3), &cache)),
                    };
                    if results.send(reply).is_err() {
                        break;
                    }
                    if let Some(ping) = ping {
                        ping.ping();
                    }
                }
            })
        {
            tracing::warn!(%error, "wallpaper picker worker unavailable");
        }
        Self {
            requests,
            replies,
            ready: Memory::new(MEMORY_CAP),
            pending: HashSet::new(),
            failed: HashSet::new(),
            revision: 0,
            epoch: 0,
            refreshing: false,
            refresh_requested: false,
            found: None,
        }
    }

    pub(super) fn refresh(&mut self, ping: Option<Ping>) {
        self.refresh_requested = true;
        self.send_refresh(ping);
    }

    fn send_refresh(&mut self, ping: Option<Ping>) {
        if self.refresh_requested && !self.refreshing && self.requests.try_send((Job::Refresh, ping)).is_ok()
        {
            self.refreshing = true;
            self.refresh_requested = false;
        }
    }

    pub(super) fn poll(&mut self) -> Option<Collection> {
        self.drain();
        self.send_refresh(None);
        self.found.take()
    }

    /// Takes in finished replies; a rescan result waits in `found` for the next `poll`.
    pub(super) fn drain(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                Reply::Collection(found) => {
                    self.refreshing = false;
                    self.epoch = self.epoch.wrapping_add(1);
                    self.ready.clear();
                    self.pending.clear();
                    self.failed.clear();
                    self.found = Some(*found);
                }
                Reply::Thumbnail(key, image) => {
                    let _removed = self.pending.remove(&key);
                    if key.0 == self.epoch {
                        if let Some(image) = image {
                            self.ready.insert(key, image);
                        } else {
                            let _inserted = self.failed.insert(key);
                        }
                    }
                }
            }
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(super) fn image(
        &mut self,
        index: usize,
        size: (u32, u32),
        source: Source,
        identity: String,
        cache: PathBuf,
        ping: Option<Ping>,
    ) -> Option<image::RgbaImage> {
        if size.0 == 0 || size.1 == 0 || size.0 > 2048 || size.1 > 2048 {
            return None;
        }
        let key = (self.epoch, index, size.0, size.1);
        if let Some(image) = self.ready.get(&key) {
            return Some(image);
        }
        if !self.failed.contains(&key)
            && !self.pending.contains(&key)
            && self
                .requests
                .try_send((
                    Job::Thumbnail {
                        key,
                        source,
                        identity,
                        cache,
                    },
                    ping,
                ))
                .is_ok()
        {
            let _inserted = self.pending.insert(key);
        }
        None
    }
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "the collection and worker share thumbnail preparation"
)]
pub(super) fn thumbnail(
    source: &Source,
    identity: &str,
    size: (u32, u32),
    cache: &Path,
) -> Option<image::RgbaImage> {
    use core::hash::{Hash as _, Hasher as _};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    ("center-area-v1", identity, source.stamp(), size).hash(&mut hash);
    let path = cache.join(format!("{:016x}.png", hash.finish()));
    if let Some(image) =
        super::wallpaper::bounded_image(&path).filter(|image| (image.width(), image.height()) == size)
    {
        if let Ok(file) = std::fs::File::options().write(true).open(&path) {
            let _touched = file.set_modified(std::time::SystemTime::now());
        }
        return Some(image.to_rgba8());
    }
    let image = source.image()?.to_rgba8();
    let (width, height) = image.dimensions();
    let (crop_width, crop_height) = if u64::from(width).saturating_mul(u64::from(size.1))
        > u64::from(height).saturating_mul(u64::from(size.0))
    {
        (
            u32::try_from(
                u64::from(height)
                    .saturating_mul(u64::from(size.0))
                    .checked_div(u64::from(size.1))?,
            )
            .ok()?,
            height,
        )
    } else {
        (
            width,
            u32::try_from(
                u64::from(width)
                    .saturating_mul(u64::from(size.1))
                    .checked_div(u64::from(size.0))?,
            )
            .ok()?,
        )
    };
    let crop = image::imageops::crop_imm(
        &image,
        width.saturating_sub(crop_width) / 2,
        height.saturating_sub(crop_height) / 2,
        crop_width.max(1),
        crop_height.max(1),
    );
    let result = area_resize(&crop.to_image(), size);
    if std::fs::create_dir_all(cache).is_ok() {
        let temporary = path.with_extension("png.tmp");
        if result
            .save_with_format(&temporary, image::ImageFormat::Png)
            .is_ok()
            && let Err(error) = std::fs::rename(&temporary, &path)
        {
            tracing::warn!(%error, "thumbnail cache replacement failed");
        }
    }
    Some(result)
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    clippy::arithmetic_side_effects,
    reason = "area integration uses bounded image coordinates and normalized channels"
)]
fn area_resize(image: &image::RgbaImage, size: (u32, u32)) -> image::RgbaImage {
    let scale_x = f64::from(image.width()) / f64::from(size.0);
    let scale_y = f64::from(image.height()) / f64::from(size.1);
    image::RgbaImage::from_fn(size.0, size.1, |x, y| {
        let left = f64::from(x) * scale_x;
        let right = f64::from(x + 1) * scale_x;
        let top = f64::from(y) * scale_y;
        let bottom = f64::from(y + 1) * scale_y;
        let mut channels = [0.0; 4];
        let mut weight = 0.0;
        for row in top.floor() as u32..(bottom.ceil() as u32).min(image.height()) {
            for col in left.floor() as u32..(right.ceil() as u32).min(image.width()) {
                let overlap = (right.min(f64::from(col + 1)) - left.max(f64::from(col)))
                    * (bottom.min(f64::from(row + 1)) - top.max(f64::from(row)));
                let pixel = image.get_pixel(col, row);
                let [_, _, _, alpha] = pixel.0;
                let alpha = f64::from(alpha) / 255.0;
                for (channel, value) in channels.iter_mut().take(3).zip(pixel.0.iter()) {
                    *channel = (f64::from(*value) * alpha).mul_add(overlap, *channel);
                }
                if let Some(channel) = channels.last_mut() {
                    *channel = (alpha * 255.0).mul_add(overlap, *channel);
                }
                weight += overlap;
            }
        }
        let [red, green, blue, alpha] = channels;
        let alpha = alpha / weight;
        image::Rgba([
            if alpha > 0.0 {
                (red / weight * 255.0 / alpha).round() as u8
            } else {
                0
            },
            if alpha > 0.0 {
                (green / weight * 255.0 / alpha).round() as u8
            } else {
                0
            },
            if alpha > 0.0 {
                (blue / weight * 255.0 / alpha).round() as u8
            } else {
                0
            },
            alpha.round() as u8,
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(side: u32) -> image::RgbaImage {
        image::RgbaImage::new(side, side)
    }

    #[test]
    fn memory_evicts_least_recently_used_within_the_cap() {
        let each = tile(10).as_raw().len();
        let mut memory = Memory::new(each * 2);
        memory.insert((0, 1, 10, 10), tile(10));
        memory.insert((0, 2, 10, 10), tile(10));
        assert!(memory.get(&(0, 1, 10, 10)).is_some());
        memory.insert((0, 3, 10, 10), tile(10));
        assert!(memory.get(&(0, 2, 10, 10)).is_none());
        assert!(memory.get(&(0, 1, 10, 10)).is_some());
        assert!(memory.get(&(0, 3, 10, 10)).is_some());
        assert_eq!(memory.bytes, each * 2);
    }

    #[test]
    fn memory_keeps_a_single_oversized_entry_and_tracks_replacement() {
        let mut memory = Memory::new(10);
        memory.insert((0, 1, 10, 10), tile(10));
        memory.insert((0, 1, 10, 10), tile(10));
        assert_eq!(memory.entries.len(), 1);
        assert_eq!(memory.bytes, tile(10).as_raw().len());
    }

    #[test]
    fn disk_prune_removes_leftovers_stale_and_oldest_over_the_cap() {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, age: u64| {
            let path = directory.path().join(name);
            std::fs::write(&path, [0_u8; 100]).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(std::time::SystemTime::now() - core::time::Duration::from_secs(age))
                .unwrap();
        };
        write("old.png", 1000);
        write("mid.png", 100);
        write("new.png", 10);
        write("half.png.tmp", 1);
        prune_disk(directory.path(), 200, core::time::Duration::from_secs(5000));
        let mut left: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|file| file.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["mid.png", "new.png"]);
        prune_disk(directory.path(), 200, core::time::Duration::from_secs(50));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
