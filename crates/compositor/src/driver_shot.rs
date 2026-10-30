//! Software capture through the same scene and overlays as the output renderer.
use crate::state::Compositor;
use core::error::Error;
use smithay::backend::{
    allocator::Fourcc,
    renderer::{Bind, ExportMem, Offscreen, damage::OutputDamageTracker, pixman::PixmanRenderer},
};
use smithay::utils::Rectangle;
use std::path::Path;

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate visibility keeps the capture adapter private"
)]
pub(crate) fn save(compositor: &Compositor, path: &Path) -> Result<(), Box<dyn Error>> {
    let output = compositor.output.as_ref().ok_or("test output unavailable")?;
    let mode = output.current_mode().ok_or("output mode unavailable")?;
    let mut renderer = PixmanRenderer::new()?;
    let mut buffer = renderer.create_buffer(Fourcc::Abgr8888, (mode.size.w, mode.size.h).into())?;
    let mut target = renderer.bind(&mut buffer)?;
    let mut overlays = crate::overlays::Overlays::default();
    let elements = overlays.elements(&mut renderer, output, compositor, "00:00")?;
    let mut damage = OutputDamageTracker::from_output(output);
    crate::wallpaper::render_output(
        output,
        &mut renderer,
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
    image.save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}
