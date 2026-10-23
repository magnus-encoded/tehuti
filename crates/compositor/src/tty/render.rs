//! GPU and software buffers share the same DRM page-flip path.

use core::{cell::RefCell, error::Error};
use std::rc::Rc;

use smithay::{
    backend::{
        allocator::{
            Allocator, Fourcc, Modifier,
            dmabuf::{AsDmabuf, Dmabuf},
            dumb::{DumbAllocator, DumbBuffer},
            gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDeviceFd, DrmNode, DrmSurface,
            dumb::{DumbFramebuffer, framebuffer_from_dumb_buffer},
            gbm::{GbmFramebuffer, framebuffer_from_bo},
        },
        renderer::{
            Bind, ImportAll, ImportDma, ImportMem, Renderer,
            damage::OutputDamageTracker,
            gles::GlesRenderer,
            multigpu::{
                ApiDevice, GraphicsApi,
                gbm::{GbmGlesBackend, GbmGlesDevice},
            },
            pixman::PixmanRenderer,
        },
    },
    output::Output,
    reexports::{drm::control::framebuffer, wayland_server::DisplayHandle},
};
use tehuti::{
    overlays::Overlays,
    rendering::{Rendering, scanout_formats},
    state::Compositor,
};

pub(super) struct Scanout<Buffer, Framebuffer> {
    framebuffer: Framebuffer,
    dmabuf: Dmabuf,
    _buffer: Buffer,
    /// The drawn-frame count when this buffer was last drawn.
    drawn_at: Option<u64>,
}

pub(super) struct Gpu {
    device: Rc<RefCell<GbmGlesDevice<GlesRenderer>>>,
    buffers: [Scanout<GbmBuffer, GbmFramebuffer>; 2],
    overlays: Overlays,
}

pub(super) struct Software {
    renderer: PixmanRenderer,
    buffers: [Scanout<DumbBuffer, DumbFramebuffer>; 2],
    overlays: Overlays,
}

/// A frame to scan out, or none when nothing on the output changed.
pub(super) struct Drawn {
    pub(super) framebuffer: framebuffer::Handle,
    pub(super) notice_included: bool,
}

/// How a frame is drawn: the drawn-frame count, and whether to redraw everything.
pub(super) struct Pass<'counter> {
    pub(super) drawn: &'counter mut u64,
    pub(super) full: bool,
}

pub(super) type DrmRendering = Rendering<Gpu, Software>;

pub(super) fn create(device_fd: &DrmDeviceFd, surface: &DrmSurface) -> Result<DrmRendering, Box<dyn Error>> {
    tehuti::rendering::initialize(
        || create_gpu(device_fd, surface),
        || create_software(device_fd, surface),
    )
}

fn create_gpu(device_fd: &DrmDeviceFd, surface: &DrmSurface) -> Result<Gpu, Box<dyn Error>> {
    let gbm = GbmDevice::new(device_fd.clone())?;
    let mut api: GbmGlesBackend<GlesRenderer, DrmDeviceFd> = GbmGlesBackend::default();
    api.add_node(DrmNode::from_file(device_fd)?, gbm.clone())?;
    let mut devices = Vec::new();
    api.enumerate(&mut devices)?;
    let mut device = devices.pop().ok_or("EGL/GLES renderer initialization failed")?;
    let formats = scanout_formats(
        device.renderer().egl_context().dmabuf_render_formats(),
        &surface.plane_info().formats,
    );
    let mut allocator = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let (width, height) = surface.pending_mode().size();
    for format in formats {
        let mut allocate = || -> Result<_, Box<dyn Error>> {
            let buffer = allocator.create_buffer(
                u32::from(width),
                u32::from(height),
                format.code,
                &[format.modifier],
            )?;
            let framebuffer = framebuffer_from_bo(device_fd, &buffer, false)?;
            let mut dmabuf = buffer.export()?;
            let target = device.renderer_mut().bind(&mut dmabuf)?;
            drop(target);
            surface.test_state([super::plane(surface, *framebuffer.as_ref())], true)?;
            Ok(Scanout {
                framebuffer,
                dmabuf,
                _buffer: buffer,
                drawn_at: None,
            })
        };
        match allocate().and_then(|first| allocate().map(|second| [first, second])) {
            Ok(buffers) => {
                return Ok(Gpu {
                    device: Rc::new(RefCell::new(device)),
                    buffers,
                    overlays: Overlays::default(),
                });
            }
            Err(error) => tracing::debug!(?format, %error, "GPU scanout format unavailable"),
        }
    }
    Err("no usable GBM/EGL scanout format".into())
}

fn create_software(device_fd: &DrmDeviceFd, surface: &DrmSurface) -> Result<Software, Box<dyn Error>> {
    let mut allocator = DumbAllocator::new(device_fd.clone());
    let (width, height) = surface.pending_mode().size();
    let mut allocate = || -> Result<_, Box<dyn Error>> {
        let buffer = allocator.create_buffer(
            u32::from(width),
            u32::from(height),
            Fourcc::Argb8888,
            &[Modifier::Linear],
        )?;
        Ok(Scanout {
            framebuffer: framebuffer_from_dumb_buffer(device_fd, &buffer, true)?,
            dmabuf: buffer.export()?,
            _buffer: buffer,
            drawn_at: None,
        })
    };
    Ok(Software {
        renderer: PixmanRenderer::new()?,
        buffers: [allocate()?, allocate()?],
        overlays: Overlays::default(),
    })
}

pub(super) fn advertise(
    rendering: &mut DrmRendering,
    display: &DisplayHandle,
    compositor: &mut Compositor,
) -> Result<(), Box<dyn Error>> {
    if let Rendering::Gles(gpu) = rendering {
        let (formats, device) =
            tehuti::rendering::client_formats(gpu.device.borrow_mut().renderer_mut(), display);
        let importer = gpu.device.clone();
        compositor.enable_dmabuf(
            display,
            formats.iter().copied(),
            device,
            move |dmabuf| match importer.borrow_mut().renderer_mut().import_dmabuf(dmabuf, None) {
                Ok(_) => true,
                Err(error) => {
                    tracing::warn!(%error, "DRM client dmabuf import failed");
                    false
                }
            },
        )?;
    }
    Ok(())
}

/// Draws the damage since `buffer` last held a frame; `None` when nothing changed.
pub(super) fn draw(
    rendering: &mut DrmRendering,
    next: bool,
    mut pass: Pass<'_>,
    output: &Output,
    damage: &mut OutputDamageTracker,
    compositor: &Compositor,
) -> Result<Option<Drawn>, Box<dyn Error>> {
    match rendering {
        Rendering::Gles(gpu) => {
            let buffer = gpu
                .buffers
                .get_mut(usize::from(next))
                .ok_or("missing GPU scanout buffer")?;
            let notice = draw_buffer(
                gpu.device.borrow_mut().renderer_mut(),
                buffer,
                &mut gpu.overlays,
                &mut pass,
                output,
                damage,
                compositor,
            )?;
            Ok(notice.map(|notice_included| Drawn {
                framebuffer: *buffer.framebuffer.as_ref(),
                notice_included,
            }))
        }
        Rendering::Pixman(software) => {
            let buffer = software
                .buffers
                .get_mut(usize::from(next))
                .ok_or("missing pixman scanout buffer")?;
            let notice = draw_buffer(
                &mut software.renderer,
                buffer,
                &mut software.overlays,
                &mut pass,
                output,
                damage,
                compositor,
            )?;
            Ok(notice.map(|notice_included| Drawn {
                framebuffer: *buffer.framebuffer.as_ref(),
                notice_included,
            }))
        }
    }
}

/// Whether the drawn frame shows a notice, or `None` when the damage tracker found no change.
fn draw_buffer<R, B, F>(
    renderer: &mut R,
    buffer: &mut Scanout<B, F>,
    overlays: &mut Overlays,
    pass: &mut Pass<'_>,
    output: &Output,
    damage: &mut OutputDamageTracker,
    compositor: &Compositor,
) -> Result<Option<bool>, Box<dyn Error>>
where
    R: Renderer + ImportAll + ImportMem + Bind<Dmabuf>,
    R::TextureId: Send + Clone + 'static,
    R::Error: 'static,
{
    let age = tehuti::pacing::buffer_age(*pass.drawn, buffer.drawn_at, pass.full);
    let clock = chrono::Local::now().format("%H:%M").to_string();
    let overlay = overlays.elements(renderer, output, compositor, &clock)?;
    let mut target = renderer.bind(&mut buffer.dmabuf)?;
    let outcome = tehuti::wallpaper::render_output_at_age(
        output,
        renderer,
        &mut target,
        compositor,
        &overlay.elements,
        damage,
        age,
    )
    .and_then(|frame| {
        let drew = frame.damage.is_some();
        frame.sync.wait()?;
        Ok(drew)
    });
    match outcome {
        Ok(false) => return Ok(None),
        Ok(true) => {}
        Err(error) => {
            // The tracker may have counted this frame, so its history and the buffers disagree.
            *damage = OutputDamageTracker::from_output(output);
            return Err(error);
        }
    }
    buffer.drawn_at = Some(*pass.drawn);
    *pass.drawn = pass.drawn.saturating_add(1);
    Ok(Some(overlay.notice_included))
}
