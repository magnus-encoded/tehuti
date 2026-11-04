//! Output capture over the ext and wlr client protocols.
use crate::state::{ClientState, Compositor};
use core::sync::atomic::{AtomicBool, Ordering};
use smithay::{
    output::Output,
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum, Weak,
        protocol::{wl_buffer::WlBuffer, wl_shm},
    },
    wayland::shm::with_buffer_contents_mut,
};
use std::sync::{Arc, Mutex};
use wayland_protocols::ext::{
    image_capture_source::v1::server::{
        ext_image_capture_source_v1::ExtImageCaptureSourceV1,
        ext_output_image_capture_source_manager_v1::{self, ExtOutputImageCaptureSourceManagerV1},
    },
    image_copy_capture::v1::server::{
        ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
        ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
        ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
        ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
    },
};
use wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture dispatch remains private to the compositor"
)]
pub(crate) fn register(display: &DisplayHandle) {
    let _source =
        display.create_global::<Compositor, ExtOutputImageCaptureSourceManagerV1, _>(1, display.clone());
    let _copy = display.create_global::<Compositor, ExtImageCopyCaptureManagerV1, _>(1, display.clone());
    let _wlr = display.create_global::<Compositor, ZwlrScreencopyManagerV1, _>(3, display.clone());
}

fn trusted(client: &Client, display: &DisplayHandle) -> bool {
    client.get_data::<ClientState>().is_some()
        && client
            .get_credentials(display)
            .is_ok_and(|credentials| credentials.uid == rustix::process::geteuid().as_raw())
}

macro_rules! capture_global {
    ($interface:ty) => {
        impl GlobalDispatch<$interface, DisplayHandle> for Compositor {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                _: &DisplayHandle,
                init: &mut DataInit<'_, Self>,
            ) {
                let _resource = init.init(resource, ());
            }
            fn can_view(client: Client, display: &DisplayHandle) -> bool {
                trusted(&client, display)
            }
        }
    };
}
capture_global!(ExtOutputImageCaptureSourceManagerV1);
capture_global!(ExtImageCopyCaptureManagerV1);
capture_global!(ZwlrScreencopyManagerV1);

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture lifecycle remains private to the compositor"
)]
#[derive(Debug, Default)]
pub(crate) struct Sessions(Vec<Weak<ExtImageCopyCaptureSessionV1>>);

#[expect(
    clippy::redundant_pub_crate,
    reason = "capture lifecycle remains private to the compositor"
)]
pub(crate) fn refresh(compositor: &mut Compositor) {
    let mut sessions = core::mem::take(&mut compositor.capture_sessions);
    sessions.0.retain(|weak| {
        let Ok(session) = weak.upgrade() else {
            return false;
        };
        if let Some(data) = session.data::<Session>() {
            data.constraints(compositor, &session);
        }
        true
    });
    compositor.capture_sessions = sessions;
}

#[derive(Debug, Clone)]
struct Source(Option<Output>);

impl Source {
    fn output<'a>(&'a self, compositor: &Compositor) -> Option<&'a Output> {
        self.0.as_ref().filter(|output| {
            compositor.space.outputs().any(|mapped| mapped == *output) && output.current_mode().is_some()
        })
    }

    fn size(&self, compositor: &Compositor) -> Option<(u32, u32)> {
        let mode = self.output(compositor)?.current_mode()?;
        Some((u32::try_from(mode.size.w).ok()?, u32::try_from(mode.size.h).ok()?))
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtOutputImageCaptureSourceManagerV1,
        request: ext_output_image_capture_source_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_output_image_capture_source_manager_v1::Request::CreateSource { source, output } = request
        {
            let _source = init.init(source, Source(Output::from_resource(&output)));
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, Source> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCaptureSourceV1,
        _: <ExtImageCaptureSourceV1 as Resource>::Request,
        _: &Source,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

#[derive(Debug, Clone)]
struct Session {
    source: Source,
    cursor: bool,
    busy: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    size: Arc<Mutex<Option<(u32, u32)>>>,
}

impl Session {
    fn new(source: Source, cursor: bool) -> Self {
        Self {
            source,
            cursor,
            busy: Arc::default(),
            stopped: Arc::default(),
            size: Arc::default(),
        }
    }

    fn constraints(&self, compositor: &Compositor, session: &ExtImageCopyCaptureSessionV1) {
        if self.stopped.load(Ordering::Relaxed) {
            return;
        }
        if let Some((width, height)) = self.source.size(compositor) {
            if let Ok(mut size) = self.size.lock() {
                if *size == Some((width, height)) {
                    return;
                }
                *size = Some((width, height));
            }
            session.buffer_size(width, height);
            session.shm_format(wl_shm::Format::Abgr8888);
            session.done();
        } else {
            self.stopped.store(true, Ordering::Relaxed);
            session.stopped();
        }
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        manager: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_image_copy_capture_manager_v1::Request::CreateSession {
                session,
                source,
                options,
            } => {
                let WEnum::Value(options) = options else {
                    let _session = init.init(session, Session::new(Source(None), false));
                    manager.post_error(
                        ext_image_copy_capture_manager_v1::Error::InvalidOption,
                        "unknown capture options",
                    );
                    return;
                };
                let source = source.data::<Source>().cloned().unwrap_or(Source(None));
                let data = Session::new(
                    source,
                    options.contains(ext_image_copy_capture_manager_v1::Options::PaintCursors),
                );
                let session = init.init(session, data.clone());
                data.constraints(state, &session);
                state.capture_sessions.0.push(session.downgrade());
            }
            ext_image_copy_capture_manager_v1::Request::CreatePointerCursorSession { session, .. } => {
                let _session = init.init(session, AtomicBool::new(false));
            }
            ext_image_copy_capture_manager_v1::Request::Destroy | _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, AtomicBool> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        cursor: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        used: &AtomicBool,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_cursor_session_v1::Request::GetCaptureSession { session } = request {
            let session = init.init(session, Session::new(Source(None), false));
            if used.swap(true, Ordering::Relaxed) {
                cursor.post_error(
                    ext_image_copy_capture_cursor_session_v1::Error::DuplicateSession,
                    "cursor session already created",
                );
                return;
            }
            session.stopped();
        }
    }
}

#[derive(Debug)]
struct ExtFrame {
    session: Session,
    resource: ExtImageCopyCaptureSessionV1,
    buffer: Mutex<Option<WlBuffer>>,
    captured: AtomicBool,
}

impl Dispatch<ExtImageCopyCaptureSessionV1, Session> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        session: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        data: &Session,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_session_v1::Request::CreateFrame { frame } = request {
            data.constraints(state, session);
            let duplicate = data.busy.swap(true, Ordering::Relaxed);
            let _frame = init.init(
                frame,
                ExtFrame {
                    session: data.clone(),
                    resource: session.clone(),
                    buffer: Mutex::default(),
                    captured: AtomicBool::new(false),
                },
            );
            if duplicate {
                session.post_error(
                    ext_image_copy_capture_session_v1::Error::DuplicateFrame,
                    "destroy the previous frame first",
                );
            }
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ExtFrame> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        frame: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &ExtFrame,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use ext_image_copy_capture_frame_v1::{Error, FailureReason, Request};
        if matches!(request, Request::Destroy) {
            return;
        }
        if data.captured.load(Ordering::Relaxed) {
            frame.post_error(Error::AlreadyCaptured, "frame already captured");
            return;
        }
        match request {
            Request::AttachBuffer { buffer } => {
                if let Ok(mut attached) = data.buffer.lock() {
                    *attached = Some(buffer);
                }
            }
            Request::DamageBuffer { x, y, width, height } => {
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    frame.post_error(Error::InvalidBufferDamage, "invalid damage rectangle");
                }
            }
            Request::Capture => {
                data.captured.store(true, Ordering::Relaxed);
                let buffer = data.buffer.lock().ok().and_then(|buffer| buffer.clone());
                let Some(buffer) = buffer else {
                    frame.post_error(Error::NoBuffer, "attach a buffer before capture");
                    return;
                };
                data.session.constraints(state, &data.resource);
                let Some(output) = data
                    .session
                    .source
                    .output(state)
                    .filter(|_| !data.session.stopped.load(Ordering::Relaxed))
                else {
                    frame.failed(FailureReason::Stopped);
                    return;
                };
                match copy(state, output, &buffer, data.session.cursor, None) {
                    Ok((width, height)) => {
                        frame.transform(output.current_transform().into());
                        frame.damage(0, 0, width, height);
                        let (high, low, nanos) = timestamp();
                        frame.presentation_time(high, low, nanos);
                        frame.ready();
                    }
                    Err(CopyError::Buffer) => frame.failed(FailureReason::BufferConstraints),
                    Err(CopyError::Render) => frame.failed(FailureReason::Unknown),
                }
            }
            Request::Destroy | _ => {}
        }
    }

    fn destroyed(
        _: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        _: &ExtImageCopyCaptureFrameV1,
        data: &ExtFrame,
    ) {
        data.session.busy.store(false, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct WlrFrame {
    source: Source,
    cursor: bool,
    region: Option<(u32, u32, u32, u32)>,
    size: Option<(u32, u32)>,
    captured: AtomicBool,
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_manager_v1::Request;
        let (frame, output, cursor, region) = match request {
            Request::CaptureOutput {
                frame,
                output,
                overlay_cursor,
            } => (frame, output, overlay_cursor != 0, None),
            Request::CaptureOutputRegion {
                frame,
                output,
                overlay_cursor,
                x,
                y,
                width,
                height,
            } => (frame, output, overlay_cursor != 0, Some((x, y, width, height))),
            Request::Destroy | _ => return,
        };
        let source = Source(Output::from_resource(&output));
        let (region, size) = region.map_or_else(
            || (None, source.size(state)),
            |rect| {
                let clipped = source.output(state).and_then(|target| clip_region(target, rect));
                (clipped, clipped.map(|(_, _, width, height)| (width, height)))
            },
        );
        let frame = init.init(
            frame,
            WlrFrame {
                source,
                cursor,
                region,
                size,
                captured: AtomicBool::new(false),
            },
        );
        if let Some((width, height)) = size.filter(|(width, _)| width.checked_mul(4).is_some()) {
            frame.buffer(wl_shm::Format::Abgr8888, width, height, width.saturating_mul(4));
            if frame.version() >= 3 {
                frame.buffer_done();
            }
        } else {
            frame.failed();
        }
    }
}

fn clip_region(output: &Output, (x, y, width, height): (i32, i32, i32, i32)) -> Option<(u32, u32, u32, u32)> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let mode = output.current_mode()?;
    let scale = output.current_scale().integer_scale();
    let transform = output.current_transform();
    let extent = transform.transform_size(mode.size);
    let left = x.checked_mul(scale)?.max(0);
    let top = y.checked_mul(scale)?.max(0);
    let right = x.checked_add(width)?.checked_mul(scale)?.min(extent.w);
    let bottom = y.checked_add(height)?.checked_mul(scale)?.min(extent.h);
    if right <= left || bottom <= top {
        return None;
    }
    let rect = smithay::utils::Rectangle::new(
        (left, top).into(),
        (right.checked_sub(left)?, bottom.checked_sub(top)?).into(),
    );
    let rect = transform.invert().transform_rect_in(rect, &extent);
    Some((
        u32::try_from(rect.loc.x).ok()?,
        u32::try_from(rect.loc.y).ok()?,
        u32::try_from(rect.size.w).ok()?,
        u32::try_from(rect.size.h).ok()?,
    ))
}

impl Dispatch<ZwlrScreencopyFrameV1, WlrFrame> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &WlrFrame,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_frame_v1::{Error, Flags, Request};
        let damage = matches!(request, Request::CopyWithDamage { .. });
        let (Request::Copy { buffer } | Request::CopyWithDamage { buffer }) = request else {
            return;
        };
        if data.captured.swap(true, Ordering::Relaxed) {
            frame.post_error(Error::AlreadyUsed, "frame already copied");
            return;
        }
        let Some(output) = data.source.output(state).filter(|_| data.size.is_some()) else {
            frame.failed();
            return;
        };
        match copy(state, output, &buffer, data.cursor, data.region) {
            Ok((width, height)) => {
                frame.flags(Flags::empty());
                if damage {
                    frame.damage(
                        0,
                        0,
                        u32::try_from(width).unwrap_or(0),
                        u32::try_from(height).unwrap_or(0),
                    );
                }
                let (high, low, nanos) = timestamp();
                frame.ready(high, low, nanos);
            }
            Err(CopyError::Buffer) => {
                frame.post_error(Error::InvalidBuffer, "buffer does not match capture constraints");
            }
            Err(CopyError::Render) => frame.failed(),
        }
    }
}

#[derive(Debug)]
enum CopyError {
    Buffer,
    Render,
}

fn copy(
    compositor: &Compositor,
    output: &Output,
    buffer: &WlBuffer,
    cursor: bool,
    region: Option<(u32, u32, u32, u32)>,
) -> Result<(i32, i32), CopyError> {
    let clock = chrono::Local::now().format("%H:%M").to_string();
    let mut image = compositor
        .output_capture
        .image(compositor, output, &clock, cursor)
        .map_err(|error| {
            tracing::warn!(%error, "screen capture failed");
            CopyError::Render
        })?;
    if let Some((x, y, width, height)) = region {
        if x.checked_add(width).is_none_or(|right| right > image.width())
            || y.checked_add(height).is_none_or(|bottom| bottom > image.height())
        {
            return Err(CopyError::Render);
        }
        image = image::imageops::crop_imm(&image, x, y, width, height).to_image();
    }
    write_image(buffer, &image)
}

fn write_image(buffer: &WlBuffer, image: &image::RgbaImage) -> Result<(i32, i32), CopyError> {
    let width = i32::try_from(image.width()).map_err(|_| CopyError::Buffer)?;
    let height = i32::try_from(image.height()).map_err(|_| CopyError::Buffer)?;
    with_buffer_contents_mut(buffer, |pointer, length, data| {
        let stride = usize::try_from(data.stride).map_err(|_| CopyError::Buffer)?;
        let offset = usize::try_from(data.offset).map_err(|_| CopyError::Buffer)?;
        let row_bytes = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(CopyError::Buffer)?;
        let rows = usize::try_from(height).map_err(|_| CopyError::Buffer)?;
        let end = rows
            .checked_sub(1)
            .and_then(|rows| rows.checked_mul(stride))
            .and_then(|bytes| bytes.checked_add(offset))
            .and_then(|bytes| bytes.checked_add(row_bytes));
        if data.width != width
            || data.height != height
            || data.format != wl_shm::Format::Abgr8888
            || stride < row_bytes
            || end.is_none_or(|end| end > length)
        {
            return Err(CopyError::Buffer);
        }
        for (row, pixels) in image.as_raw().chunks_exact(row_bytes).enumerate() {
            let address = row
                .checked_mul(stride)
                .and_then(|bytes| bytes.checked_add(offset))
                .and_then(|offset| pointer.addr().checked_add(offset))
                .ok_or(CopyError::Buffer)?;
            // The kernel reports EFAULT for a truncated pool instead of faulting the compositor.
            let written = nix::sys::uio::process_vm_writev(
                nix::unistd::getpid(),
                &[std::io::IoSlice::new(pixels)],
                &[nix::sys::uio::RemoteIoVec {
                    base: address,
                    len: pixels.len(),
                }],
            )
            .map_err(|_| CopyError::Buffer)?;
            if written != pixels.len() {
                return Err(CopyError::Buffer);
            }
        }
        Ok((width, height))
    })
    .map_err(|_| CopyError::Buffer)?
}

fn timestamp() -> (u32, u32, u32) {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
    (
        u32::try_from(seconds >> 32).unwrap_or(0),
        u32::try_from(seconds & 0xffff_ffff).unwrap_or(0),
        u32::try_from(now.tv_nsec).unwrap_or(0),
    )
}
