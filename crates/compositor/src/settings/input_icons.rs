//! Original Input artwork rasterised and retained at physical pixel sizes.

use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock, Mutex},
};

type Key = (&'static str, u32, u32, [u8; 4]);
type Cache = BTreeMap<Key, Arc<image::RgbaImage>>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "physical icon dimensions fit the output"
)]
#[expect(clippy::float_arithmetic, reason = "SVG transforms scale to physical pixels")]
pub(super) fn raster(
    icon: &'static str,
    width: u32,
    height: u32,
    colour: [u8; 4],
) -> Option<Arc<image::RgbaImage>> {
    let key = (icon, width, height, colour);
    let mut cache = CACHE.lock().ok()?;
    if let Some(image) = cache.get(&key) {
        return Some(Arc::clone(image));
    }
    let source = match icon {
        "all" => include_str!("../../assets/input/all.svg"),
        "auto" => include_str!("../../assets/input/auto.svg"),
        "bluetooth" => include_str!("../../assets/input/bluetooth.svg"),
        "builtin" => include_str!("../../assets/input/builtin.svg"),
        "cable" => include_str!("../../assets/input/cable.svg"),
        "dongle" => include_str!("../../assets/input/dongle.svg"),
        "dualsense" => include_str!("../../assets/input/dualsense.svg"),
        "generic-pad" => include_str!("../../assets/input/generic-pad.svg"),
        "headset" => include_str!("../../assets/input/headset.svg"),
        "mic" => include_str!("../../assets/input/mic.svg"),
        "network" => include_str!("../../assets/input/network.svg"),
        "phone" => include_str!("../../assets/input/phone.svg"),
        "switch" => include_str!("../../assets/input/switch.svg"),
        "usb-mic" => include_str!("../../assets/input/usb-mic.svg"),
        "xbox" => include_str!("../../assets/input/xbox.svg"),
        _ => return None,
    };
    let [blue, green, red, _] = colour;
    let source = source.replace("#e1f0f4", &format!("#{red:02x}{green:02x}{blue:02x}"));
    let tree = resvg::usvg::Tree::from_str(&source, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(width as f32 / 100.0, height as f32 / 100.0),
        &mut pixmap.as_mut(),
    );
    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.blue(), pixel.green(), pixel.red(), pixel.alpha()]
        })
        .collect();
    let image = Arc::new(image::RgbaImage::from_raw(width, height, pixels)?);
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&image));
    Some(image)
}
