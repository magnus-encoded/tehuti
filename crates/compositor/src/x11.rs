//! X11 and Wayland placement share the same desktop space.
use super::{Compositor, LayoutWindow, Window, WlSurface};
use smithay::{
    delegate_xwayland_shell,
    input::keyboard::KeyboardTarget,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        seat::WaylandFocus,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm,
        xwm::{Reorder, ResizeEdge, WmWindowProperty, WmWindowType, XwmHandler, XwmId},
    },
};

#[derive(Debug, Default)]
pub(super) struct Floating {
    pub(super) windows: Vec<LayoutWindow>,
    pub(super) focus: Option<LayoutWindow>,
}

impl Compositor {
    pub(super) fn configure_x11(
        window: &LayoutWindow,
        space: &smithay::desktop::Space<LayoutWindow>,
        tile: &tehuti_core::layout::TileView,
    ) {
        if let Some(x11) = window.x11_surface() {
            let geometry = smithay::utils::Rectangle::new(
                space.element_location(window).unwrap_or_default(),
                window.layout_size(),
            );
            if x11.geometry() != geometry
                && let Err(error) = x11.configure(geometry)
            {
                tracing::warn!(%error, "X11 configure failed");
            }
            if let Err(error) = x11.set_fullscreen(tile.fullscreen_requested) {
                tracing::warn!(%error, "X11 fullscreen state failed");
            }
        }
    }

    pub(crate) fn clear_x11(&mut self) {
        let identities: Vec<_> = self
            .layout_windows
            .iter()
            .filter(|(_, window)| window.x11_surface().is_some())
            .map(|(identity, _)| *identity)
            .collect();
        for identity in identities {
            self.remove_layout_window(identity, false);
        }
        let floating: Vec<_> = self
            .floating
            .windows
            .iter()
            .filter(|window| window.x11_surface().is_some())
            .cloned()
            .collect();
        for window in floating {
            self.space.unmap_elem(&window);
        }
        self.floating
            .windows
            .retain(|window| window.x11_surface().is_none());
        self.floating.focus = None;
        self.x11.manager = None;
        self.sync_window_focus();
    }

    fn x11_floats(window: &X11Surface) -> bool {
        crate::xwayland::should_float(
            window.is_override_redirect(),
            window.is_transient_for().is_some(),
            matches!(
                window.window_type(),
                Some(WmWindowType::Dialog | WmWindowType::Splash | WmWindowType::Utility)
            ),
            window.min_size().map(|size| (size.w, size.h)),
            window.max_size().map(|size| (size.w, size.h)),
        )
    }

    fn map_x11(&mut self, surface: &X11Surface) {
        if self
            .layout_windows
            .iter()
            .any(|(_, window)| window.x11_surface() == Some(surface))
            || self
                .floating
                .windows
                .iter()
                .any(|window| window.x11_surface() == Some(surface))
        {
            return;
        }
        let window = LayoutWindow::new(Window::new_x11_window(surface.clone()));
        if Self::x11_floats(surface) && (!surface.is_fullscreen() || surface.is_override_redirect()) {
            window.set_size(surface.geometry().size);
            if !surface.is_override_redirect() {
                self.floating.focus = Some(window.clone());
            }
            self.floating.windows.push(window);
            self.place_layout();
        } else {
            self.add_tiled(window, surface.is_fullscreen());
        }
    }

    fn add_tiled(&mut self, window: LayoutWindow, fullscreen: bool) {
        self.fill_output();
        let identity = tehuti_core::layout::WindowId(self.next_window);
        self.next_window = self.next_window.saturating_add(1);
        let rules = tehuti_core::layout::WindowRules {
            open_fullscreen: fullscreen,
            ..Default::default()
        };
        match self.layout.add_window(identity, rules) {
            Ok(()) => {
                self.floating.focus = None;
                self.layout_windows.push((identity, window));
                self.place_layout();
            }
            Err(error) => tracing::warn!(%error, "window layout failed"),
        }
    }

    pub(super) fn reclassify_wayland(&mut self, root: &WlSurface) {
        let Some(window) = self.window_for_root(root).cloned() else {
            return;
        };
        if self.window_fullscreen(&window) {
            return;
        }
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let transient = toplevel.parent().is_some();
        let float = smithay::wayland::compositor::with_states(root, |states| {
            let mut cached = states
                .cached_state
                .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
            let constraints = *cached.current();
            drop(cached);
            crate::xwayland::should_float(
                false,
                transient,
                false,
                Some((constraints.min_size.w, constraints.min_size.h)),
                Some((constraints.max_size.w, constraints.max_size.h)),
            )
        });
        if float
            && let Some(index) = self
                .layout_windows
                .iter()
                .position(|(_, candidate)| candidate == &window)
        {
            let (identity, removed) = self.layout_windows.remove(index);
            if let Err(error) = self.layout.remove_window(identity) {
                tracing::warn!(%error, "floating transition failed");
            }
            toplevel.with_pending_state(|pending| {
                pending.size = None;
                for state in [
                    super::xdg_toplevel::State::TiledLeft,
                    super::xdg_toplevel::State::TiledRight,
                    super::xdg_toplevel::State::TiledTop,
                    super::xdg_toplevel::State::TiledBottom,
                ] {
                    let _changed = pending.states.unset(state);
                }
            });
            self.floating.focus = Some(removed.clone());
            self.floating.windows.push(removed);
            self.place_layout();
        } else if !float
            && let Some(index) = self
                .floating
                .windows
                .iter()
                .position(|candidate| candidate == &window)
        {
            let removed = self.floating.windows.remove(index);
            self.add_tiled(removed, false);
        }
        self.place_floating();
    }

    pub(super) fn place_floating(&mut self) {
        let area = self
            .output
            .as_ref()
            .and_then(|output| self.space.output_geometry(output))
            .unwrap_or_else(|| Rectangle::from_size((1280, 720).into()));
        for window in &self.floating.windows {
            let mut size = window
                .x11_surface()
                .map_or_else(|| window.client_size(), |x11| x11.geometry().size);
            if size.w <= 0 || size.h <= 0 {
                size = (640, 480).into();
            }
            window.set_size(size);
            let location = (
                area.loc.x.saturating_add(area.size.w.saturating_sub(size.w) / 2),
                area.loc.y.saturating_add(area.size.h.saturating_sub(size.h) / 2),
            );
            if let Some(x11) = window.x11_surface()
                && !x11.is_override_redirect()
                && x11.geometry() != Rectangle::new(location.into(), size)
                && let Err(error) = x11.configure(Rectangle::new(location.into(), size))
            {
                tracing::warn!(%error, "floating configure failed");
            }
            self.space.map_element(window.clone(), location, false);
            self.space.raise_element(window, false);
        }
    }

    pub(super) fn remove_floating(&mut self, surface: &WlSurface) {
        let removed: Vec<_> = self
            .floating
            .windows
            .iter()
            .filter(|window| window.wl_surface().is_some_and(|root| root.as_ref() == surface))
            .cloned()
            .collect();
        for window in removed {
            self.space.unmap_elem(&window);
            self.floating.windows.retain(|candidate| candidate != &window);
            if self.floating.focus.as_ref() == Some(&window) {
                self.floating.focus = None;
            }
        }
        self.sync_window_focus();
    }

    pub(super) fn activate_window(&self, selected: Option<&LayoutWindow>) {
        for window in self
            .layout_windows
            .iter()
            .map(|(_, window)| window)
            .chain(self.floating.windows.iter())
        {
            if window.set_activated(selected == Some(window))
                && let Some(toplevel) = window.toplevel()
                && toplevel.is_initial_configure_sent()
            {
                let _serial = toplevel.send_pending_configure();
            }
        }
    }

    pub(super) fn focus_x11(&mut self, window: &X11Surface) {
        let seat = self.seat.clone();
        self.keyboard
            .clone()
            .with_pressed_keysyms(|keys| window.enter(&seat, self, keys, SERIAL_COUNTER.next_serial()));
    }

    pub(super) fn update_x11_title(&mut self, window: &X11Surface) {
        if window.wl_surface().is_some() && self.keyboard.current_focus() == window.wl_surface() {
            self.handle(&tehuti_protocol::Event::Focus {
                app_id: Some(window.class()),
                title: window.title(),
            });
        }
    }

    fn unmap_x11(&mut self, window: &X11Surface) {
        if let Some(surface) = window.wl_surface() {
            self.remove_floating(&surface);
        }
        // An X11 unmap can arrive after its Wayland surface has disappeared.
        let removed: Vec<_> = self
            .floating
            .windows
            .iter()
            .filter(|candidate| candidate.x11_surface() == Some(window))
            .cloned()
            .collect();
        for candidate in removed {
            self.space.unmap_elem(&candidate);
            self.floating.windows.retain(|held| held != &candidate);
            if self.floating.focus.as_ref() == Some(&candidate) {
                self.floating.focus = None;
            }
        }
        let identity = self
            .layout_windows
            .iter()
            .find(|(_, candidate)| candidate.x11_surface() == Some(window))
            .map(|(identity, _)| *identity);
        if let Some(identity) = identity {
            self.remove_layout_window(identity, false);
        }
    }

    fn reclassify_x11(&mut self, surface: &X11Surface) {
        if surface.is_fullscreen() {
            return;
        }
        let tiled = self
            .layout_windows
            .iter()
            .position(|(_, candidate)| candidate.x11_surface() == Some(surface));
        if Self::x11_floats(surface) {
            if let Some(index) = tiled {
                let (identity, removed) = self.layout_windows.remove(index);
                if let Err(error) = self.layout.remove_window(identity) {
                    tracing::warn!(%error, "floating transition failed");
                }
                self.floating.focus = Some(removed.clone());
                self.floating.windows.push(removed);
                self.place_layout();
            }
        } else if let Some(index) = self
            .floating
            .windows
            .iter()
            .position(|candidate| candidate.x11_surface() == Some(surface))
        {
            let removed = self.floating.windows.remove(index);
            self.add_tiled(removed, false);
        }
    }

    fn window_fullscreen(&self, window: &LayoutWindow) -> bool {
        let identity = self
            .layout_windows
            .iter()
            .find(|(_, candidate)| candidate == window)
            .map(|(identity, _)| *identity);
        self.layout
            .snapshot()
            .outputs
            .iter()
            .flat_map(|output| &output.workspaces)
            .flat_map(|workspace| &workspace.columns)
            .flat_map(|column| &column.tiles)
            .any(|tile| Some(tile.window) == identity && tile.fullscreen_requested)
    }

    pub(super) fn request_fullscreen(&mut self, window: &LayoutWindow, fullscreen: bool) {
        if window.x11_surface().is_some_and(X11Surface::is_override_redirect) {
            return;
        }
        if fullscreen
            && let Some(index) = self
                .floating
                .windows
                .iter()
                .position(|candidate| candidate == window)
        {
            let removed = self.floating.windows.remove(index);
            self.add_tiled(removed, true);
            return;
        }
        let identity = self
            .layout_windows
            .iter()
            .find(|(_, candidate)| candidate == window)
            .map(|(identity, _)| *identity);
        if let Some(identity) = identity {
            if let Err(error) = self.layout.focus_window(identity) {
                tracing::warn!(%error, "fullscreen focus failed");
                return;
            }
            if self.window_fullscreen(window) != fullscreen {
                self.apply_layout(tehuti_protocol::LayoutAction::ToggleFullscreen);
            }
        }
        if !fullscreen {
            if let Some(surface) = window.x11_surface() {
                self.reclassify_x11(surface);
            } else if let Some(toplevel) = window.toplevel() {
                self.reclassify_wayland(toplevel.wl_surface());
            }
        }
    }

    fn x11_fullscreen(&mut self, surface: &X11Surface, fullscreen: bool) {
        let window = self
            .layout_windows
            .iter()
            .map(|(_, window)| window)
            .chain(self.floating.windows.iter())
            .find(|window| window.x11_surface() == Some(surface))
            .cloned();
        if let Some(window) = window {
            self.request_fullscreen(&window, fullscreen);
        }
    }
}

impl XWaylandShellHandler for Compositor {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.x11.shell
    }
    fn surface_associated(&mut self, _: XwmId, _: WlSurface, _: X11Surface) {
        self.place_layout();
    }
}

impl XwmHandler for Compositor {
    #[expect(
        clippy::expect_used,
        reason = "Smithay calls XwmHandler only while its registered WM exists"
    )]
    fn xwm_state(&mut self, _: XwmId) -> &mut X11Wm {
        self.x11.manager.as_mut().expect("registered X11 window manager")
    }
    fn new_window(&mut self, _: XwmId, _: X11Surface) {}
    fn new_override_redirect_window(&mut self, _: XwmId, _: X11Surface) {}
    fn map_window_request(&mut self, _: XwmId, window: X11Surface) {
        if let Err(error) = window.set_mapped(true) {
            tracing::warn!(%error, "X11 map failed");
            return;
        }
        self.map_x11(&window);
    }
    fn mapped_override_redirect_window(&mut self, _: XwmId, window: X11Surface) {
        self.map_x11(&window);
    }
    fn unmapped_window(&mut self, _: XwmId, window: X11Surface) {
        self.unmap_x11(&window);
    }
    fn destroyed_window(&mut self, _: XwmId, window: X11Surface) {
        self.unmap_x11(&window);
    }
    fn configure_request(
        &mut self,
        _: XwmId,
        window: X11Surface,
        _: Option<i32>,
        _: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _: Option<Reorder>,
    ) {
        if Self::x11_floats(&window) {
            let mut geometry = window.geometry();
            if let Some(width) = width.and_then(|value| i32::try_from(value).ok()) {
                geometry.size.w = width.max(1);
            }
            if let Some(height) = height.and_then(|value| i32::try_from(value).ok()) {
                geometry.size.h = height.max(1);
            }
            if let Err(error) = window.configure(geometry) {
                tracing::warn!(%error, "X11 resize failed");
            }
        }
        self.place_layout();
    }
    fn configure_notify(&mut self, _: XwmId, _: X11Surface, _: Rectangle<i32, Logical>, _: Option<u32>) {
        self.place_floating();
    }
    fn property_notify(&mut self, _: XwmId, window: X11Surface, property: WmWindowProperty) {
        if matches!(
            property,
            WmWindowProperty::NormalHints | WmWindowProperty::TransientFor | WmWindowProperty::WindowType
        ) {
            self.reclassify_x11(&window);
        }
        if matches!(property, WmWindowProperty::Title | WmWindowProperty::Class) {
            self.update_x11_title(&window);
        }
    }
    fn fullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        self.x11_fullscreen(&window, true);
    }
    fn unfullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        self.x11_fullscreen(&window, false);
    }
    fn resize_request(&mut self, _: XwmId, _: X11Surface, _: u32, _: ResizeEdge) {}
    fn move_request(&mut self, _: XwmId, _: X11Surface, _: u32) {}
    fn disconnected(&mut self, _: XwmId) {
        self.x11.failed = true;
    }
}
delegate_xwayland_shell!(Compositor);
