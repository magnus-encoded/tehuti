//! Saved screenshots preserve the orientation of the rendered scene.

#[cfg(test)]
mod tests {
    use smithay::{
        backend::{
            allocator::Fourcc,
            renderer::{Bind, Frame, Offscreen, Renderer, pixman::PixmanRenderer},
        },
        utils::{Rectangle, Transform},
    };

    #[test]
    fn saved_screenshot_keeps_top_and_bottom_rows() -> Result<(), Box<dyn core::error::Error>> {
        for transform in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            let size = transform.transform_size(smithay::utils::Size::<i32, smithay::utils::Physical>::from(
                (2, 3),
            ));
            let mut renderer = PixmanRenderer::new()?;
            let mut buffer = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;
            let mut target = renderer.bind(&mut buffer)?;
            let mut frame = renderer.render(&mut target, size, transform)?;
            for (location, color) in [
                ((0, 0), [1.0, 0.0, 0.0, 1.0]),
                ((1, 0), [1.0, 1.0, 0.0, 1.0]),
                ((0, 1), [0.0, 1.0, 0.0, 1.0]),
                ((1, 1), [0.0, 1.0, 1.0, 1.0]),
                ((0, 2), [0.0, 0.0, 1.0, 1.0]),
                ((1, 2), [1.0, 0.0, 1.0, 1.0]),
            ] {
                frame.clear(color.into(), &[Rectangle::new(location.into(), (1, 1).into())])?;
            }
            frame.finish()?.wait()?;
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("screen.png");
            tehuti::system::capture(&mut renderer, &target, (size.w, size.h), transform, &path)?;
            let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
            let mut reader = decoder.read_info()?;
            let mut pixels = vec![0; reader.output_buffer_size().ok_or("missing PNG size")?];
            let info = reader.next_frame(&mut pixels)?;
            assert_eq!((info.width, info.height), (2, 3), "dimensions for {transform:?}");
            assert_eq!(
                pixels.get(..info.buffer_size()).ok_or("incomplete PNG")?,
                &[
                    255, 0, 0, 255, 255, 255, 0, 255, 0, 255, 0, 255, 0, 255, 255, 255, 0, 0, 255, 255, 255,
                    0, 255, 255
                ],
                "orientation for {transform:?}"
            );
        }
        Ok(())
    }
}
