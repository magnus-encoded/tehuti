//! Framebuffer row order at the screenshot boundary.
use smithay::utils::Transform;

#[derive(Debug, Clone, Copy)]
pub enum Readback {
    Gles,
    Pixman,
}

impl Readback {
    /// Converts renderer rows into the output's top-to-bottom image order.
    #[must_use]
    pub fn orient(
        self,
        mut image: image::RgbaImage,
        transform: Transform,
        mapping_flipped: bool,
    ) -> image::RgbaImage {
        let bottom_up = match self {
            Self::Gles => !mapping_flipped,
            Self::Pixman => mapping_flipped,
        };
        if (transform == Transform::Flipped180) != bottom_up {
            image::imageops::flip_vertical_in_place(&mut image);
        }
        image
    }
}
