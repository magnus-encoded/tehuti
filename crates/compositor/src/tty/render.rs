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
            element::{Kind, memory::MemoryRenderBufferRenderElement},
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
    rendering::{Rendering, scanout_formats},
    state::Compositor,
};

pub(super) struct Scanout<Buffer, Framebuffer> {
    framebuffer: Framebuffer,
    dmabuf: Dmabuf,
    _buffer: Buffer,
    bar: tehuti::bar::Buffer,
    keyboard: tehuti::osk::Buffer,
}

pub(super) struct Gpu {
    device: Rc<RefCell<GbmGlesDevice<GlesRenderer>>>,
    buffers: [Scanout<GbmBuffer, GbmFramebuffer>; 2],
}

pub(super) struct Software {
    renderer: PixmanRenderer,
    buffers: [Scanout<DumbBuffer, DumbFramebuffer>; 2],
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
                bar: tehuti::bar::Buffer::default(),
                keyboard: tehuti::osk::Buffer::default(),
            })
        };
        match allocate().and_then(|first| allocate().map(|second| [first, second])) {
            Ok(buffers) => {
                return Ok(Gpu {
                    device: Rc::new(RefCell::new(device)),
                    buffers,
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
            bar: tehuti::bar::Buffer::default(),
            keyboard: tehuti::osk::Buffer::default(),
        })
    };
    Ok(Software {
        renderer: PixmanRenderer::new()?,
        buffers: [allocate()?, allocate()?],
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

pub(super) fn draw(
    rendering: &mut DrmRendering,
    next: bool,
    output: &Output,
    damage: &mut OutputDamageTracker,
    compositor: &Compositor,
) -> Result<(framebuffer::Handle, bool), Box<dyn Error>> {
    match rendering {
        Rendering::Gles(gpu) => {
            let buffer = gpu
                .buffers
                .get_mut(usize::from(next))
                .ok_or("missing GPU scanout buffer")?;
            let notice = draw_buffer(
                gpu.device.borrow_mut().renderer_mut(),
                &mut buffer.dmabuf,
                &mut buffer.bar,
                &mut buffer.keyboard,
                output,
                damage,
                compositor,
            )?;
            Ok((*buffer.framebuffer.as_ref(), notice))
        }
        Rendering::Pixman(software) => {
            let buffer = software
                .buffers
                .get_mut(usize::from(next))
                .ok_or("missing pixman scanout buffer")?;
            let notice = draw_buffer(
                &mut software.renderer,
                &mut buffer.dmabuf,
                &mut buffer.bar,
                &mut buffer.keyboard,
                output,
                damage,
                compositor,
            )?;
            Ok((*buffer.framebuffer.as_ref(), notice))
        }
    }
}

fn draw_buffer<R>(
    renderer: &mut R,
    dmabuf: &mut Dmabuf,
    bar: &mut tehuti::bar::Buffer,
    keyboard: &mut tehuti::osk::Buffer,
    output: &Output,
    damage: &mut OutputDamageTracker,
    compositor: &Compositor,
) -> Result<bool, Box<dyn Error>>
where
    R: Renderer + ImportAll + ImportMem + Bind<Dmabuf>,
    R::TextureId: Send + Clone + 'static,
    R::Error: 'static,
{
    let mut target = renderer.bind(dmabuf)?;
    let notice = compositor
        .visible_notice_text()
        .map(|text| tehuti::notice::notice_buffer(&text));
    let mut elements = notice
        .as_ref()
        .map(|buffer| {
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (16.0, 16.0),
                buffer,
                None,
                None,
                None,
                Kind::Unspecified,
            )
        })
        .transpose()?
        .into_iter()
        .collect::<Vec<_>>();
    let notice_included = !elements.is_empty();
    let clock = chrono::Local::now().format("%H:%M").to_string();
    if let Some(element) = bar.element(renderer, output, compositor.voice_bar(), &clock)? {
        elements.push(element);
    }
    if let Some(element) = keyboard.element(renderer, output, compositor.on_screen_keyboard())? {
        elements.push(element);
    }
    if let Some(element) = tehuti::settings::element(renderer, compositor, output)? {
        elements.insert(0, element);
    }
    if let Some(location) = compositor.cursor_on(output) {
        elements.insert(
            0,
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location,
                &tehuti::notice::cursor_buffer(),
                None,
                None,
                None,
                Kind::Cursor,
            )?,
        );
    }
    let frame =
        tehuti::wallpaper::render_output(output, renderer, &mut target, compositor, &elements, damage)?;
    frame.sync.wait()?;
    Ok(notice_included && frame.damage.is_some())
}
