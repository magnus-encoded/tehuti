#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};
    use smithay::utils::Transform;
    use tehuti::readback::Readback;

    #[test]
    fn readback_places_known_colours_in_the_top_and_bottom_rows() {
        let top = Rgba([12, 34, 56, 255]);
        let bottom = Rgba([78, 90, 123, 255]);
        for (readback, transform, mapping_flipped, rows) in [
            (Readback::Gles, Transform::Normal, false, [bottom, top]),
            (Readback::Gles, Transform::Normal, true, [top, bottom]),
            (Readback::Gles, Transform::Flipped180, false, [top, bottom]),
            (Readback::Gles, Transform::Flipped180, true, [bottom, top]),
            (Readback::Pixman, Transform::Normal, false, [top, bottom]),
            (Readback::Pixman, Transform::Normal, true, [bottom, top]),
            (Readback::Pixman, Transform::Flipped180, false, [bottom, top]),
            (Readback::Pixman, Transform::Flipped180, true, [top, bottom]),
        ] {
            let mut image = RgbaImage::new(1, 2);
            image.put_pixel(0, 0, rows[0]);
            image.put_pixel(0, 1, rows[1]);
            let image = readback.orient(image, transform, mapping_flipped);
            assert_eq!(*image.get_pixel(0, 0), top, "{readback:?} {transform:?}");
            assert_eq!(*image.get_pixel(0, 1), bottom, "{readback:?} {transform:?}");
        }
    }
}
