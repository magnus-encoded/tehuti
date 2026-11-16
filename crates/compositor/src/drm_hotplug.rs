//! Session lifetime and connector rescans for DRM device notifications.

use core::ops::ControlFlow;
use smithay::backend::udev::UdevEvent;

/// Handles a device notification, rescanning outputs or ending the session.
pub fn handle_event(event: &UdevEvent, device_id: libc::dev_t, rescan: impl FnOnce()) -> ControlFlow<()> {
    match event {
        UdevEvent::Changed { device_id: changed } if *changed == device_id => {
            rescan();
            ControlFlow::Continue(())
        }
        UdevEvent::Removed { device_id: removed } if *removed == device_id => ControlFlow::Break(()),
        UdevEvent::Added { .. } | UdevEvent::Changed { .. } | UdevEvent::Removed { .. } => {
            ControlFlow::Continue(())
        }
    }
}

/// Advertises the DRM output at `mode`, reusing the previous output when one exists.
///
/// A client-visible `wl_output` global must outlive a connector rescan that finds the same
/// output: removing it makes the X11 helper panic and drops every X11 window.
#[must_use]
pub fn publish_output(
    display: &smithay::reexports::wayland_server::DisplayHandle,
    previous: Option<(
        smithay::output::Output,
        smithay::reexports::wayland_server::backend::GlobalId,
    )>,
    mode: smithay::output::Mode,
    scale: i32,
) -> (
    smithay::output::Output,
    smithay::reexports::wayland_server::backend::GlobalId,
) {
    use smithay::output::{Output, PhysicalProperties, Subpixel};
    use smithay::utils::Transform;
    if let Some((output, global)) = previous {
        output.change_current_state(Some(mode), Some(smithay::utils::Transform::Normal), None, None);
        crate::scale::apply(&output, mode, scale);
        output.set_preferred(mode);
        return (output, global);
    }
    let output = Output::new(
        "tehuti-drm".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Tehuti".into(),
            model: "DRM".into(),
        },
    );
    let global = output.create_global::<crate::state::Compositor>(display);
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    crate::scale::apply(&output, mode, scale);
    output.set_preferred(mode);
    (output, global)
}
