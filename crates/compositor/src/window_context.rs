//! Launcher ancestry for context selection, independent of window focus identity.

use smithay::{
    reexports::wayland_server::{DisplayHandle, Resource, protocol::wl_surface::WlSurface},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

pub fn ancestors(surface: &WlSurface, windows: &[WlSurface], display: &DisplayHandle) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut visited_surfaces = vec![surface.clone()];
    let mut parent_surface = parent(surface);
    while let Some(ancestor) = parent_surface {
        if visited_surfaces.contains(&ancestor) {
            break;
        }
        visited_surfaces.push(ancestor.clone());
        ancestors.extend(app_id(&ancestor));
        parent_surface = parent(&ancestor);
    }
    let mut visited_processes = Vec::new();
    let mut parent_process = pid(surface, display).and_then(parent_pid);
    while let Some(process) = parent_process {
        if process <= 1 || visited_processes.contains(&process) {
            break;
        }
        visited_processes.push(process);
        for window in windows {
            if pid(window, display) == Some(process) {
                ancestors.extend(app_id(window));
            }
        }
        if let Ok(executable) = std::fs::read_link(format!("/proc/{process}/exe")) {
            ancestors.extend(
                executable
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned),
            );
        }
        parent_process = parent_pid(process);
    }
    ancestors
}

fn pid(surface: &WlSurface, display: &DisplayHandle) -> Option<i32> {
    surface
        .client()?
        .get_credentials(display)
        .ok()
        .map(|credentials| credentials.pid)
}

fn parent_pid(process: i32) -> Option<i32> {
    let status = std::fs::read_to_string(format!("/proc/{process}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:")?.trim().parse().ok())
}

fn app_id(surface: &WlSurface) -> Option<String> {
    with_states(surface, |states| {
        let attributes = states.data_map.get::<XdgToplevelSurfaceData>()?;
        let attributes = attributes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        attributes.app_id.clone()
    })
}

fn parent(surface: &WlSurface) -> Option<WlSurface> {
    with_states(surface, |states| {
        let attributes = states.data_map.get::<XdgToplevelSurfaceData>()?;
        let attributes = attributes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        attributes.parent.clone()
    })
}
