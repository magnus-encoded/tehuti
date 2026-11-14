//! Whole-output screen capture for clients through ext-image-copy-capture-v1 into shm buffers.
//!
//! Trust rule: every client on Tehuti's socket may capture, since the socket is the user's own.
//! No frame is copied while a secret confirmation is on screen.

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols::ext::{
            image_capture_source::v1::server::{
                ext_image_capture_source_v1::{self, ExtImageCaptureSourceV1},
                ext_output_image_capture_source_manager_v1::{self, ExtOutputImageCaptureSourceManagerV1},
            },
            image_copy_capture::v1::server::{
                ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
                ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason},
                ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
                ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
            },
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::{wl_buffer::WlBuffer, wl_output::Transform, wl_shm::Format},
        },
    },
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::state::Compositor;

/// The shm formats a capture can fill, in the order clients hear them.
pub const FORMATS: [Format; 4] = [
    Format::Argb8888,
    Format::Xrgb8888,
    Format::Abgr8888,
    Format::Xbgr8888,
];

/// Why a frame could not be written into a client buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferMismatch {
    Size,
    Format,
    Stride,
    Length,
}

/// Writes an RGBA image into a client buffer laid out as `format` with `stride` bytes per row.
///
/// # Errors
/// The buffer has another size or format than the image, or is too short for its stride.
pub fn encode(
    image: &image::RgbaImage,
    format: Format,
    size: (u32, u32),
    stride: usize,
    destination: &mut [u8],
) -> Result<(), BufferMismatch> {
    if size != image.dimensions() {
        return Err(BufferMismatch::Size);
    }
    if !FORMATS.contains(&format) {
        return Err(BufferMismatch::Format);
    }
    let swap = matches!(format, Format::Argb8888 | Format::Xrgb8888);
    let row = usize::try_from(size.0)
        .map_err(|_| BufferMismatch::Size)?
        .saturating_mul(4);
    if stride < row {
        return Err(BufferMismatch::Stride);
    }
    let rows = usize::try_from(size.1).map_err(|_| BufferMismatch::Size)?;
    if rows > 0 && destination.len() < stride.saturating_mul(rows.saturating_sub(1)).saturating_add(row) {
        return Err(BufferMismatch::Length);
    }
    for (source, target) in image
        .as_raw()
        .chunks_exact(row.max(1))
        .zip(destination.chunks_mut(stride.max(1)))
    {
        for (pixel, out) in source
            .as_chunks::<4>()
            .0
            .iter()
            .zip(target.as_chunks_mut::<4>().0)
        {
            let [red, green, blue, alpha] = *pixel;
            // wl_shm formats are little-endian: Argb8888 stores B, G, R, A in memory.
            *out = if swap { [blue, green, red, alpha] } else { *pixel };
        }
    }
    Ok(())
}

pub(crate) fn register(display: &DisplayHandle) {
    let _sources = display.create_global::<Compositor, ExtOutputImageCaptureSourceManagerV1, _>(1, ());
    let _capture = display.create_global::<Compositor, ExtImageCopyCaptureManagerV1, _>(1, ());
}

#[derive(Debug, Default)]
struct FrameState {
    buffer: Option<WlBuffer>,
    captured: bool,
}

#[derive(Debug, Default)]
struct SessionState {
    frame: Option<ExtImageCopyCaptureFrameV1>,
}

/// The output a session captures and the mode size it announced; `None` for sessions that can only stop.
#[derive(Debug)]
pub struct SessionData {
    output: Option<Output>,
    size: Option<(i32, i32)>,
    stopped: AtomicBool,
    state: Mutex<SessionState>,
}

impl SessionData {
    fn new(output: Option<Output>) -> Self {
        let size = output
            .as_ref()
            .and_then(Output::current_mode)
            .map(|mode| (mode.size.w, mode.size.h));
        Self {
            output,
            size,
            stopped: AtomicBool::new(false),
            state: Mutex::default(),
        }
    }

    /// Whether the session's output is still Tehuti's output at the size the client was told.
    fn current(&self, compositor: &Compositor) -> bool {
        !self.stopped.load(Ordering::Relaxed)
            && self.output.is_some()
            && compositor.output.as_ref() == self.output.as_ref()
            && self
                .output
                .as_ref()
                .and_then(Output::current_mode)
                .map(|mode| (mode.size.w, mode.size.h))
                == self.size
    }
}

#[derive(Debug)]
pub struct FrameData {
    session: ExtImageCopyCaptureSessionV1,
    state: Mutex<FrameState>,
}

/// Live capture sessions, told `stopped` once their output goes away or changes mode.
#[derive(Debug, Default)]
#[expect(clippy::redundant_pub_crate, reason = "stored in compositor state")]
pub(crate) struct Sessions(Vec<ExtImageCopyCaptureSessionV1>);

impl Compositor {
    pub(crate) fn stop_stale_captures(&mut self) {
        let sessions = core::mem::take(&mut self.capture_sessions.0);
        self.capture_sessions.0 = sessions
            .into_iter()
            .filter(|session| {
                let Some(data) = session.data::<SessionData>() else {
                    return false;
                };
                if !session.is_alive() || data.stopped.load(Ordering::Relaxed) {
                    return false;
                }
                if data.current(self) {
                    return true;
                }
                data.stopped.store(true, Ordering::Relaxed);
                session.stopped();
                false
            })
            .collect();
    }
}

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let _manager = data_init.init(resource, ());
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
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_output_image_capture_source_manager_v1::Request::CreateSource { source, output } = request
        {
            let _source = data_init.init(source, Output::from_resource(&output));
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, Option<Output>> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCaptureSourceV1,
        _: ext_image_capture_source_v1::Request,
        _: &Option<Output>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for Compositor {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let _manager = data_init.init(resource, ());
    }
}

fn source_output(source: &ExtImageCaptureSourceV1) -> Option<Output> {
    source.data::<Option<Output>>().cloned().flatten()
}

fn announce(session: &ExtImageCopyCaptureSessionV1, output: Option<&Output>) {
    let size = output
        .and_then(Output::current_mode)
        .and_then(|mode| Some((u32::try_from(mode.size.w).ok()?, u32::try_from(mode.size.h).ok()?)));
    let Some((width, height)) = size else {
        session.stopped();
        return;
    };
    session.buffer_size(width, height);
    for format in FORMATS {
        session.shm_format(format);
    }
    session.done();
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        manager: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_image_copy_capture_manager_v1::Request::CreateSession {
                session,
                source,
                options,
            } => {
                if options.into_result().is_err() {
                    manager.post_error(
                        ext_image_copy_capture_manager_v1::Error::InvalidOption,
                        "unknown capture option",
                    );
                    return;
                }
                let output = source_output(&source);
                let session = data_init.init(session, SessionData::new(output.clone()));
                announce(&session, output.as_ref());
                state.capture_sessions.0.push(session);
            }
            ext_image_copy_capture_manager_v1::Request::CreatePointerCursorSession { session, .. } => {
                let _session = data_init.init(session, Mutex::new(false));
            }
            ext_image_copy_capture_manager_v1::Request::Destroy => {}
            _ => tracing::debug!("unknown image copy capture request"),
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, Mutex<bool>> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        cursor: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        taken: &Mutex<bool>,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_cursor_session_v1::Request::GetCaptureSession { session } = request {
            let Ok(mut taken) = taken.lock() else { return };
            if core::mem::replace(&mut *taken, true) {
                cursor.post_error(
                    ext_image_copy_capture_cursor_session_v1::Error::DuplicateSession,
                    "capture session already taken",
                );
                return;
            }
            // Cursor images are not offered yet: the session stops at once.
            let session = data_init.init(session, SessionData::new(None));
            session.stopped();
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, SessionData> for Compositor {
    fn request(
        _: &mut Self,
        _: &Client,
        session: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        data: &SessionData,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_session_v1::Request::CreateFrame { frame } = request {
            let Ok(mut state) = data.state.lock() else { return };
            if state.frame.as_ref().is_some_and(Resource::is_alive) {
                session.post_error(
                    ext_image_copy_capture_session_v1::Error::DuplicateFrame,
                    "a frame is already pending",
                );
                return;
            }
            state.frame = Some(data_init.init(
                frame,
                FrameData {
                    session: session.clone(),
                    state: Mutex::default(),
                },
            ));
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, FrameData> for Compositor {
    fn request(
        state: &mut Self,
        _: &Client,
        frame: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Ok(mut frame_state) = data.state.lock() else {
            return;
        };
        if frame_state.captured && !matches!(request, ext_image_copy_capture_frame_v1::Request::Destroy) {
            frame.post_error(
                ext_image_copy_capture_frame_v1::Error::AlreadyCaptured,
                "frame was already captured",
            );
            return;
        }
        match request {
            ext_image_copy_capture_frame_v1::Request::AttachBuffer { buffer } => {
                frame_state.buffer = Some(buffer);
            }
            ext_image_copy_capture_frame_v1::Request::Capture => {
                let Some(buffer) = frame_state.buffer.clone() else {
                    frame.post_error(
                        ext_image_copy_capture_frame_v1::Error::NoBuffer,
                        "no buffer attached",
                    );
                    return;
                };
                frame_state.captured = true;
                drop(frame_state);
                let Some(session) = data.session.data::<SessionData>() else {
                    frame.failed(FailureReason::Stopped);
                    return;
                };
                match capture(state, session, &buffer) {
                    Ok(()) => ready(frame, session.output.as_ref()),
                    Err(reason) => frame.failed(reason),
                }
            }
            ext_image_copy_capture_frame_v1::Request::DamageBuffer { .. }
            | ext_image_copy_capture_frame_v1::Request::Destroy => {}
            _ => tracing::debug!("unknown capture frame request"),
        }
    }
}

fn capture(compositor: &Compositor, session: &SessionData, buffer: &WlBuffer) -> Result<(), FailureReason> {
    if !session.current(compositor) {
        return Err(FailureReason::Stopped);
    }
    // The refusal reads as `unknown` on purpose: the client must not learn a secret prompt is open.
    if compositor.secret_confirmation_text().is_some() {
        tracing::info!("screen capture refused while a secret confirmation is on screen");
        return Err(FailureReason::Unknown);
    }
    let clock = chrono::Local::now().format("%H:%M").to_string();
    let image = crate::driver_shot::frame(compositor, &clock).map_err(|error| {
        tracing::warn!(%error, "screen capture failed");
        FailureReason::Unknown
    })?;
    write(&image, buffer)
}

fn write(image: &image::RgbaImage, buffer: &WlBuffer) -> Result<(), FailureReason> {
    let written = smithay::wayland::shm::with_buffer_contents_mut(buffer, |pointer, length, data| {
        let size = (u32::try_from(data.width).ok()?, u32::try_from(data.height).ok()?);
        let offset = usize::try_from(data.offset).ok()?;
        let stride = usize::try_from(data.stride).ok()?;
        let format = data.format;
        if offset > length {
            return None;
        }
        let pool = pool_bytes(pointer, length);
        encode(image, format, size, stride, pool.get_mut(offset..)?).ok()
    });
    match written {
        Ok(Some(())) => Ok(()),
        Ok(None) | Err(_) => Err(FailureReason::BufferConstraints),
    }
}

#[expect(unsafe_code, reason = "shm pool memory is only reachable as a raw pointer")]
const fn pool_bytes<'pool>(pointer: *mut u8, length: usize) -> &'pool mut [u8] {
    // SAFETY: called only inside `with_buffer_contents_mut`, where Smithay keeps the pool mapped for
    // `length` bytes and guards SIGBUS from a client shrinking it; the slice does not outlive the closure.
    unsafe { core::slice::from_raw_parts_mut(pointer, length) }
}

fn ready(frame: &ExtImageCopyCaptureFrameV1, output: Option<&Output>) {
    let size = output
        .and_then(Output::current_mode)
        .map_or((0, 0), |mode| (mode.size.w, mode.size.h));
    frame.transform(Transform::Normal);
    frame.damage(0, 0, size.0, size.1);
    let now = core::time::Duration::from(smithay::utils::Clock::<smithay::utils::Monotonic>::new().now());
    let seconds = now.as_secs();
    frame.presentation_time(
        u32::try_from(seconds >> 32).unwrap_or(u32::MAX),
        u32::try_from(seconds & u64::from(u32::MAX)).unwrap_or(u32::MAX),
        now.subsec_nanos(),
    );
    frame.ready();
}
