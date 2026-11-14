//! Bright focus edges inside the committed window geometry.
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

/// Step marks along the bottom (width) or left (height) focus edge while a size stepper is open.
///
/// Every step gets a short notch inside the window; the chosen step's notch is
/// longer and bright, so the border itself shows the change.
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate visibility satisfies unreachable_pub for a helper the sibling state module calls"
)]
pub(crate) fn step_marks(
    bounds: Rectangle<i32, Logical>,
    scale: f64,
    axis: tehuti_core::adjuster::SizeAxis,
    chosen: usize,
    identities: &[Id],
) -> Vec<SolidColorRenderElement> {
    let bounds = bounds.to_physical_precise_round::<f64, i32>(scale);
    let thickness = Size::<i32, Logical>::from((2, 2))
        .to_physical_precise_round::<f64, i32>(scale)
        .w
        .max(1);
    let last = i32::try_from(identities.len().saturating_sub(1))
        .unwrap_or(1)
        .max(1);
    let (left, top, width, height) = (bounds.loc.x, bounds.loc.y, bounds.size.w, bounds.size.h);
    identities
        .iter()
        .enumerate()
        .map(|(index, identity)| {
            let step = i32::try_from(index).unwrap_or_default();
            let selected = index == chosen;
            let length = thickness.saturating_mul(if selected { 12 } else { 6 });
            let wide = thickness.saturating_mul(if selected { 3 } else { 2 });
            let geometry = match axis {
                tehuti_core::adjuster::SizeAxis::Width => {
                    let span = width.saturating_sub(wide);
                    let x = left.saturating_add(span.saturating_mul(step).saturating_div(last));
                    let y = top.saturating_add(height).saturating_sub(length);
                    Rectangle::new((x, y).into(), (wide, length).into())
                }
                tehuti_core::adjuster::SizeAxis::Height => {
                    let span = height.saturating_sub(wide);
                    let y = top.saturating_add(span.saturating_mul(step).saturating_div(last));
                    Rectangle::new((left, y).into(), (length, wide).into())
                }
            };
            let shade = if selected { 1.0 } else { 0.55 };
            SolidColorRenderElement::new(
                identity.clone(),
                geometry,
                0,
                [shade, shade, shade, 1.0],
                Kind::Unspecified,
            )
        })
        .collect()
}
