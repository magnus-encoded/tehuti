//! Folder rescans and physical wallpaper thumbnails prepared away from drawing.

use super::wallpaper::{Collection, Source};
use smithay::reexports::calloop::ping::Ping;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::mpsc,
};

type Key = (u64, usize, u32, u32);

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
pub(super) struct Picker {
    requests: mpsc::SyncSender<(Job, Option<Ping>)>,
    replies: mpsc::Receiver<Reply>,
    ready: HashMap<Key, image::RgbaImage>,
    pending: HashSet<Key>,
    pub(super) revision: u64,
    epoch: u64,
    refreshing: bool,
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
                        Job::Refresh => Reply::Collection(Box::new(Collection::from_env())),
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
            ready: HashMap::new(),
            pending: HashSet::new(),
            revision: 0,
            epoch: 0,
            refreshing: false,
        }
    }

    pub(super) fn refresh(&mut self, ping: Option<Ping>) {
        if !self.refreshing && self.requests.try_send((Job::Refresh, ping)).is_ok() {
            self.refreshing = true;
        }
    }

    pub(super) fn poll(&mut self) -> Option<Collection> {
        let mut collection = None;
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                Reply::Collection(found) => {
                    self.refreshing = false;
                    self.epoch = self.epoch.wrapping_add(1);
                    self.ready.clear();
                    self.pending.clear();
                    collection = Some(*found);
                }
                Reply::Thumbnail(key, image) => {
                    let _removed = self.pending.remove(&key);
                    if key.0 == self.epoch
                        && let Some(image) = image
                    {
                        let bytes: usize = self.ready.values().map(|image| image.as_raw().len()).sum();
                        if bytes.saturating_add(image.as_raw().len()) > 64 * 1024 * 1024 {
                            self.ready.clear();
                        }
                        let _previous = self.ready.insert(key, image);
                    }
                }
            }
            self.revision = self.revision.wrapping_add(1);
        }
        collection
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
            return Some(image.clone());
        }
        if !self.pending.contains(&key)
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

pub(super) fn thumbnail(
    source: &Source,
    identity: &str,
    size: (u32, u32),
    cache: &Path,
) -> Option<image::RgbaImage> {
    use std::hash::{Hash as _, Hasher as _};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    ("center-area-v1", identity, source.stamp(), size).hash(&mut hash);
    let path = cache.join(format!("{:016x}.png", hash.finish()));
    if let Some(image) =
        super::wallpaper::bounded_image(&path).filter(|image| (image.width(), image.height()) == size)
    {
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
        {
            if let Err(error) = std::fs::rename(&temporary, &path) {
                tracing::warn!(%error, "thumbnail cache replacement failed");
            }
        }
    }
    Some(result)
}

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
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
                    *channel += f64::from(*value) * alpha * overlap;
                }
                if let Some(channel) = channels.last_mut() {
                    *channel += alpha * 255.0 * overlap;
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
