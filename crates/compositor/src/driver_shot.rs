//! Capture through the output renderer, with software capture for headless outputs.
use crate::{readback::Readback, state::Compositor};
use core::error::Error;
use smithay::backend::{
    allocator::Fourcc,
    renderer::{
        Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, TextureMapping,
        damage::OutputDamageTracker, pixman::PixmanRenderer,
    },
};
use smithay::utils::Rectangle;
use std::path::Path;

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate visibility keeps the capture adapter private"
)]
pub(crate) fn save(compositor: &Compositor, path: &Path) -> Result<(), Box<dyn Error>> {
    save_with_clock(compositor, path, "00:00")
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture adapters remain private to the compositor"
)]
pub(crate) fn save_with_clock(
    compositor: &Compositor,
    path: &Path,
    clock: &str,
) -> Result<(), Box<dyn Error>> {
    frame(compositor, clock)?.save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}

#[expect(clippy::redundant_pub_crate, reason = "shared output capture stays internal")]
pub(super) fn frame(compositor: &Compositor, clock: &str) -> Result<image::RgbaImage, Box<dyn Error>> {
    if let Some(capture) = &compositor.capture_source.0 {
        capture(compositor, clock)
    } else {
        capture::<_, smithay::reexports::pixman::Image<'static, 'static>>(
            &mut PixmanRenderer::new()?,
            compositor,
            clock,
            Readback::Pixman,
        )
    }
}

#[expect(clippy::redundant_pub_crate, reason = "used by the rendering module")]
pub(super) fn capture<R, B>(
    renderer: &mut R,
    compositor: &Compositor,
    clock: &str,
    readback: Readback,
) -> Result<image::RgbaImage, Box<dyn Error>>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<B> + Bind<B> + ExportMem,
    R::TextureId: Send + Clone + 'static,
    R::Error: 'static,
{
    let output = compositor.output.as_ref().ok_or("output unavailable")?;
    let mode = output.current_mode().ok_or("output mode unavailable")?;
    let mut buffer: B = renderer.create_buffer(Fourcc::Abgr8888, (mode.size.w, mode.size.h).into())?;
    let mut target = renderer.bind(&mut buffer)?;
    let mut overlays = crate::overlays::Overlays::default();
    overlays.settle_icons(compositor);
    let elements = overlays.elements(renderer, output, compositor, clock)?;
    let mut damage = OutputDamageTracker::from_output(output);
    crate::wallpaper::render_output(
        output,
        renderer,
        &mut target,
        compositor,
        &elements.elements,
        &mut damage,
    )?
    .sync
    .wait()?;
    let mapping = renderer.copy_framebuffer(
        &target,
        Rectangle::from_size((mode.size.w, mode.size.h).into()),
        Fourcc::Abgr8888,
    )?;
    let pixels = renderer.map_texture(&mapping)?.to_vec();
    let image = image::RgbaImage::from_raw(u32::try_from(mode.size.w)?, u32::try_from(mode.size.h)?, pixels)
        .ok_or("invalid capture dimensions")?;
    Ok(readback.orient(image, output.current_transform(), mapping.flipped()))
}
