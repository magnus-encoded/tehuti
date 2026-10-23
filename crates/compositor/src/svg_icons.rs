//! Optional, bounded SVG artwork decoding.

use image::RgbaImage;

/// Renders self-contained SVG artwork into a square RGBA image.
#[must_use]
#[expect(
    clippy::float_arithmetic,
    reason = "SVG fitting uses floating-point transforms."
)]
pub fn rasterize_svg(bytes: &[u8], size: u32) -> Option<RgbaImage> {
    if bytes.len() > 1_048_576 || size == 0 || size > 1024 {
        return None;
    }
    let options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..resvg::usvg::Options::default()
    };
    let tree = resvg::usvg::Tree::from_str(core::str::from_utf8(bytes).ok()?, &options).ok()?;
    if tree.size().width() > 4096.0 || tree.size().height() > 4096.0 {
        return None;
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let target = u16::try_from(size).ok()?;
    let scale = (f32::from(target) / tree.size().width()).min(f32::from(target) / tree.size().height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    RgbaImage::from_raw(size, size, pixels)
}
