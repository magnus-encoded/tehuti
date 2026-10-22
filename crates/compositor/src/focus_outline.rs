//! Bright focus edges inside the assigned window box.
use smithay::{
    backend::renderer::element::{Id, Kind, solid::SolidColorRenderElement},
    utils::{Logical, Rectangle, Size},
};

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate visibility satisfies unreachable_pub for a helper the sibling state module calls"
)]
pub(crate) fn elements(
    bounds: Rectangle<i32, Logical>,
    scale: f64,
    identities: &[Id; 4],
) -> Vec<SolidColorRenderElement> {
    let bounds = bounds.to_physical_precise_round::<f64, i32>(scale);
    let thickness = Size::<i32, Logical>::from((2, 2))
        .to_physical_precise_round::<f64, i32>(scale)
        .w
        .min(bounds.size.w)
        .min(bounds.size.h);
    let (left, top, width, height) = (bounds.loc.x, bounds.loc.y, bounds.size.w, bounds.size.h);
    let inner_height = height.saturating_sub(thickness.saturating_mul(2));
    let inner_top = top.saturating_add(thickness);
    let right = left.saturating_add(width).saturating_sub(thickness);
    let bottom = top.saturating_add(height).saturating_sub(thickness);
    [
        Rectangle::new((left, top).into(), (width, thickness).into()),
        Rectangle::new((left, bottom).into(), (width, thickness).into()),
        Rectangle::new((left, inner_top).into(), (thickness, inner_height).into()),
        Rectangle::new((right, inner_top).into(), (thickness, inner_height).into()),
    ]
    .into_iter()
    .zip(identities)
    .map(|(geometry, identity)| {
        SolidColorRenderElement::new(
            identity.clone(),
            geometry,
            0,
            [1.0, 1.0, 1.0, 1.0],
            Kind::Unspecified,
        )
    })
    .collect()
}
