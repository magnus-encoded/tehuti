//! A flat stand-in for windows whose buffers the renderer cannot import.
use smithay::{
    backend::renderer::{
        ImportAll, Renderer,
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::import_surface_tree,
    },
    desktop::Window,
    output::Output,
    utils::Rectangle,
    wayland::{compositor::with_states, seat::WaylandFocus, shell::xdg::XdgToplevelSurfaceData},
};

use crate::state::Compositor;
use core::sync::atomic::{AtomicBool, Ordering};

/// Marks a surface whose last import failed, so each failure run warns once.
struct ImportFailed(AtomicBool);

/// The colour drawn where a window's buffer failed to import.
pub const RGBA: [u8; 4] = [51, 51, 51, 255];

/// Imports every mapped window on `output`; each one that fails gets a placeholder and a warning.
pub fn elements<R>(renderer: &mut R, compositor: &Compositor, output: &Output) -> Vec<SolidColorRenderElement>
where
    R: Renderer + ImportAll,
    R::TextureId: 'static,
{
    let Some(area) = compositor.space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();
    compositor
        .space
        .elements_for_output(output)
        .filter(|window| window.mapped())
        .filter_map(|window| {
            let surface = window.wl_surface()?;
            let imported = import_surface_tree(renderer, &surface);
            let first_failure = with_states(&surface, |states| {
                let _inserted = states
                    .data_map
                    .insert_if_missing_threadsafe(|| ImportFailed(AtomicBool::new(false)));
                states
                    .data_map
                    .get::<ImportFailed>()
                    .is_some_and(|failed| !failed.0.swap(imported.is_err(), Ordering::Relaxed))
            });
            let error = imported.err()?;
            if first_failure {
                let name = window_name(window);
                tracing::warn!(window = %name, %error, "window buffer import failed; drawing a placeholder");
            }
            let location = compositor.space.element_location(window)?;
            let location = (
                location.x.saturating_sub(area.loc.x),
                location.y.saturating_sub(area.loc.y),
            );
            let bounds = Rectangle::new(location.into(), window.client_geometry().size)
                .to_physical_precise_round::<f64, i32>(scale);
            Some(SolidColorRenderElement::new(
                Id::new(),
                bounds,
                0,
                colour(),
                Kind::Unspecified,
            ))
        })
        .collect()
}

#[expect(clippy::float_arithmetic, reason = "renderer colours are normalised floats")]
fn colour() -> [f32; 4] {
    RGBA.map(|channel| f32::from(channel) / 255.0)
}

fn window_name(window: &Window) -> String {
    window
        .toplevel()
        .and_then(|toplevel| {
            with_states(toplevel.wl_surface(), |states| {
                let attributes = states.data_map.get::<XdgToplevelSurfaceData>()?;
                let attributes = attributes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                attributes.app_id.clone().or_else(|| attributes.title.clone())
            })
        })
        .unwrap_or_else(|| "unnamed window".into())
}
