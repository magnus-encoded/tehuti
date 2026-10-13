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
        if formats.is_empty() || self.dmabuf.global.is_some() {
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
        if self.dmabuf.import.as_mut().is_some_and(|import| import(&dmabuf)) {
            if let Err(error) = notifier.successful::<Self>() {
                tracing::debug!(%error, "dmabuf client disconnected during import");
            }
        } else {
            notifier.failed();
        }
    }
}

smithay::delegate_dmabuf!(Compositor);
