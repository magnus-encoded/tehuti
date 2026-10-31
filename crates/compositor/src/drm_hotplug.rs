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
