//! Renderer selection, scanout formats, and client buffer negotiation.

use smithay::{
    backend::{
        allocator::{Format, Fourcc, dmabuf::Dmabuf, format::FormatSet},
        egl::EGLDevice,
        renderer::{ImportDma, ImportEgl, gles::GlesRenderer},
    },
    reexports::wayland_server::DisplayHandle,
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};

use crate::state::Compositor;
use core::cell::RefCell;
use std::rc::{Rc, Weak};

#[derive(Debug)]
pub enum Rendering<Gpu, Software> {
    Gles(Gpu),
    Pixman(Software),
}

/// Tries GPU initialization before allocating any software resources.
///
/// # Errors
/// Returns the software initialization error if both paths fail.
pub fn initialize<Gpu, Software, GpuError: core::fmt::Display, SoftwareError>(
    gpu: impl FnOnce() -> Result<Gpu, GpuError>,
    software: impl FnOnce() -> Result<Software, SoftwareError>,
) -> Result<Rendering<Gpu, Software>, SoftwareError> {
    match gpu() {
        Ok(renderer) => {
            tracing::info!("DRM rendering with GLES on GBM/EGL");
            Ok(Rendering::Gles(renderer))
        }
        Err(error) => {
            tracing::warn!(%error, "GPU initialization failed; falling back to CPU pixman rendering (shm clients only)");
            software().map(Rendering::Pixman)
        }
    }
}

/// Formats usable by both GLES rendering and the primary scanout plane.
#[must_use]
pub fn scanout_formats(renderer: &FormatSet, plane: &FormatSet) -> Vec<Format> {
    [Fourcc::Xrgb8888, Fourcc::Argb8888]
        .into_iter()
        .flat_map(|code| renderer.iter().filter(move |format| format.code == code))
        .filter(|format| plane.contains(format))
        .copied()
        .collect()
}

type ImportDmabuf = Box<dyn FnMut(&Dmabuf) -> bool>;

pub(crate) struct DmabufSupport {
    state: DmabufState,
    global: Option<DmabufGlobal>,
    import: Option<ImportDmabuf>,
}

impl Default for DmabufSupport {
    fn default() -> Self {
        Self {
            state: DmabufState::new(),
            global: None,
            import: None,
        }
    }
}

impl core::fmt::Debug for DmabufSupport {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DmabufSupport")
            .field("global", &self.global)
            .finish_non_exhaustive()
    }
}

impl Compositor {
    /// Advertises the renderer's import formats, with feedback when its device is known.
    /// Later calls keep the global and route imports to the newest renderer.
    /// An empty format set leaves dmabuf unavailable.
    ///
    /// # Errors
    /// Returns an error if the feedback format table cannot be created.
    pub fn enable_dmabuf(
        &mut self,
        display: &DisplayHandle,
        formats: impl IntoIterator<Item = Format>,
        render_device: Option<libc::dev_t>,
        import: impl FnMut(&Dmabuf) -> bool + 'static,
    ) -> Result<(), std::io::Error> {
        let formats: Vec<_> = formats.into_iter().collect();
        if formats.is_empty() {
            return Ok(());
        }
        if self.dmabuf.global.is_some() {
            // A replaced renderer must take over imports; the old one never cleans its cache again.
            self.dmabuf.import = Some(Box::new(import));
            return Ok(());
        }
        let global = if let Some(device) = render_device {
            let feedback = DmabufFeedbackBuilder::new(device, formats).build()?;
            self.dmabuf
                .state
                .create_global_with_default_feedback::<Self>(display, &feedback)
        } else {
            tracing::warn!("EGL render node unavailable; advertising dmabuf v3 without device feedback");
            self.dmabuf.state.create_global::<Self>(display, formats)
        };
        self.dmabuf.global = Some(global);
        self.dmabuf.import = Some(Box::new(import));
        Ok(())
    }
}

impl Compositor {
    /// Advertises dmabuf formats and imports each client buffer on `device`, held weakly.
    ///
    /// The weak hold lets a renderer replaced after hotplug or a mode change drop with its
    /// texture cache; the backend re-advertises the new renderer as soon as it is built.
    ///
    /// # Errors
    /// Returns an error if the feedback format table cannot be created.
    pub fn enable_dmabuf_on<D: 'static>(
        &mut self,
        display: &DisplayHandle,
        formats: impl IntoIterator<Item = Format>,
        render_device: Option<libc::dev_t>,
        device: &Rc<RefCell<D>>,
        import: impl Fn(&mut D, &Dmabuf) -> bool + 'static,
    ) -> Result<(), std::io::Error> {
        let device: Weak<RefCell<D>> = Rc::downgrade(device);
        self.enable_dmabuf(display, formats, render_device, move |dmabuf| {
            let Some(device) = device.upgrade() else {
                tracing::warn!("client dmabuf import reached a replaced renderer; rejecting it");
                return false;
            };
            let mut device = device.borrow_mut();
            import(&mut device, dmabuf)
        })
    }

    /// Imports a client dmabuf on the current renderer; `false` when none accepts it.
    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        self.dmabuf.import.as_mut().is_some_and(|import| import(dmabuf))
    }
}

/// Captures the output scene using the same GLES renderer that imports client buffers.
///
/// # Errors
/// Returns an error when rendering or framebuffer readback fails.
pub fn capture_frame(
    renderer: &mut GlesRenderer,
    compositor: &Compositor,
    clock: &str,
) -> Result<image::RgbaImage, Box<dyn core::error::Error>> {
    crate::driver_shot::capture::<_, smithay::backend::renderer::gles::GlesRenderbuffer>(
        renderer, compositor, clock,
    )
}

/// Binds legacy EGL clients and discovers the renderer's dma-buf capabilities.
#[must_use]
pub fn client_formats(
    renderer: &mut GlesRenderer,
    display: &DisplayHandle,
) -> (FormatSet, Option<libc::dev_t>) {
    if let Err(error) = renderer.bind_wl_display(display) {
        tracing::warn!(%error, "EGL wl_drm binding unavailable");
    }
    let device = EGLDevice::device_for_display(renderer.egl_context().display())
        .and_then(|device| device.try_get_render_node());
    let render_device = match device {
        Ok(node) => node.map(|node| node.dev_id()),
        Err(error) => {
            tracing::warn!(%error, "EGL render node discovery failed");
            None
        }
    };
    (renderer.dmabuf_formats(), render_device)
}

impl DmabufHandler for Compositor {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf.state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.import_dmabuf(&dmabuf) {
            if let Err(error) = notifier.successful::<Self>() {
                tracing::debug!(%error, "dmabuf client disconnected during import");
            }
        } else {
            notifier.failed();
        }
    }
}

smithay::delegate_dmabuf!(Compositor);

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::{
        backend::allocator::{Modifier, dmabuf::DmabufFlags},
        reexports::wayland_server::Display,
    };

    #[derive(Default)]
    struct Device {
        imports: usize,
    }

    fn dmabuf() -> Dmabuf {
        let file = tempfile::tempfile().unwrap();
        let mut builder = Dmabuf::builder((4, 4), Fourcc::Argb8888, Modifier::Linear, DmabufFlags::empty());
        assert!(builder.add_plane(file.into(), 0, 0, 16));
        builder.build().unwrap()
    }

    fn formats() -> [Format; 1] {
        [Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::Linear,
        }]
    }

    fn accept(device: &mut Device, _: &Dmabuf) -> bool {
        device.imports = device.imports.saturating_add(1);
        true
    }

    #[test]
    fn a_replaced_renderer_stops_receiving_imports_and_is_released() {
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let first = Rc::new(RefCell::new(Device::default()));
        compositor
            .enable_dmabuf_on(&display.handle(), formats(), None, &first, accept)
            .unwrap();
        assert!(compositor.import_dmabuf(&dmabuf()));
        let second = Rc::new(RefCell::new(Device::default()));
        compositor
            .enable_dmabuf_on(&display.handle(), formats(), None, &second, accept)
            .unwrap();
        assert!(compositor.import_dmabuf(&dmabuf()));
        assert!(compositor.import_dmabuf(&dmabuf()));
        assert_eq!(
            first.borrow().imports,
            1,
            "the replaced renderer imports nothing more"
        );
        assert_eq!(second.borrow().imports, 2, "imports reach the newest renderer");
        let released = Rc::downgrade(&first);
        drop(first);
        assert!(
            released.upgrade().is_none(),
            "the compositor holds no replaced renderer"
        );
    }

    #[test]
    fn a_failed_import_is_rejected_and_retains_nothing() {
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let device = Rc::new(RefCell::new(Device::default()));
        compositor
            .enable_dmabuf_on(&display.handle(), formats(), None, &device, |_, _| false)
            .unwrap();
        let buffer = dmabuf();
        let weak = buffer.weak();
        assert!(!compositor.import_dmabuf(&buffer));
        drop(buffer);
        assert!(weak.is_gone(), "nothing keeps a buffer whose import failed");
        assert_eq!(
            Rc::strong_count(&device),
            1,
            "the compositor holds the renderer weakly"
        );
    }
}
