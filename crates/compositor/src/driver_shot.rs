//! Software capture through the same scene and overlays as the output renderer.
use crate::state::Compositor;
use core::error::Error;
use smithay::backend::{
    allocator::Fourcc,
    renderer::{
        Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, damage::OutputDamageTracker,
        pixman::PixmanRenderer,
    },
};
use smithay::utils::{Rectangle, Transform};
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
    let output = compositor.output.as_ref().ok_or("output unavailable")?;
    let mut image = image(compositor, output, clock, true)?;
    if output.current_transform() == Transform::Flipped180 {
        image::imageops::flip_vertical_in_place(&mut image);
    }
    image.save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture adapters remain private to the compositor"
)]
pub(crate) fn image(
    compositor: &Compositor,
    output: &smithay::output::Output,
    clock: &str,
    paint_cursor: bool,
) -> Result<image::RgbaImage, Box<dyn Error>> {
    let mode = output.current_mode().ok_or("output mode unavailable")?;
    let mut renderer = PixmanRenderer::new()?;
    let mut buffer = renderer.create_buffer(Fourcc::Abgr8888, (mode.size.w, mode.size.h).into())?;
    render_image(
        &mut renderer,
        &mut buffer,
        compositor,
        output,
        clock,
        paint_cursor,
    )
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture rendering remains inside the compositor"
)]
pub(crate) fn render_image<R, Buffer>(
    renderer: &mut R,
    buffer: &mut Buffer,
    compositor: &Compositor,
    output: &smithay::output::Output,
    clock: &str,
    paint_cursor: bool,
) -> Result<image::RgbaImage, Box<dyn Error>>
where
    R: Renderer + Bind<Buffer> + ExportMem + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: 'static,
{
    let mode = output.current_mode().ok_or("output mode unavailable")?;
    let mut target = renderer.bind(buffer)?;
    let mut overlays = crate::overlays::Overlays::default();
    let elements = overlays.elements_with_cursor(renderer, output, compositor, clock, paint_cursor)?;
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
    Ok(image)
}
