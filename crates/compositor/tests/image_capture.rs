//! Clients capture a whole output through ext-image-copy-capture-v1 into shm buffers.

#[cfg(test)]
mod image_capture {
    use core::{cell::RefCell, time::Duration};
    use std::{
        io::{Read, Seek, SeekFrom, Write},
        os::{fd::AsFd, unix::net::UnixStream},
        rc::Rc,
        sync::Arc,
        thread,
        time::Instant,
    };

    use proptest::prelude::*;
    use smithay::reexports::wayland_server::{Display, protocol::wl_shm::Format as ServerFormat};
    use tehuti::{
        image_capture::{BufferMismatch, FORMATS, encode},
        state::{ClientState, Compositor},
    };
    use wayland_client::{
        Connection, Dispatch, QueueHandle, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_buffer::WlBuffer,
            wl_output::WlOutput,
            wl_registry::WlRegistry,
            wl_shm::{self, WlShm},
            wl_shm_pool::WlShmPool,
        },
    };
    use wayland_protocols::ext::{
        image_capture_source::v1::client::{
            ext_image_capture_source_v1::ExtImageCaptureSourceV1,
            ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
        },
        image_copy_capture::v1::client::{
            ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
            ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
            ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
        },
    };

    fn pixel(format: ServerFormat, bytes: [u8; 4]) -> [u8; 4] {
        let [first, second, third, fourth] = bytes;
        if matches!(format, ServerFormat::Argb8888 | ServerFormat::Xrgb8888) {
            [third, second, first, fourth]
        } else {
            bytes
        }
    }

    fn generated_image() -> impl Strategy<Value = image::RgbaImage> {
        (1_u32..12, 1_u32..12).prop_flat_map(|(width, height)| {
            proptest::collection::vec(
                any::<u8>(),
                usize::try_from(width.saturating_mul(height).saturating_mul(4)).unwrap_or(0),
            )
            .prop_map(move |bytes| image::RgbaImage::from_raw(width, height, bytes).unwrap_or_default())
        })
    }

    proptest! {
        #[test]
        fn encoded_buffers_decode_to_the_frame_and_keep_row_padding(
            image in generated_image(),
            format in proptest::sample::select(FORMATS.to_vec()),
            padding in 0_usize..9,
            filler in any::<u8>(),
        ) {
            let (width, height) = image.dimensions();
            let row = usize::try_from(width).unwrap_or(0).saturating_mul(4);
            let stride = row.saturating_add(padding);
            let mut buffer = vec![filler; stride.saturating_mul(usize::try_from(height).unwrap_or(0))];
            prop_assert_eq!(encode(&image, format, (width, height), stride, &mut buffer), Ok(()));
            for (y, line) in buffer.chunks(stride).enumerate() {
                let (pixels, pad) = line.split_at(row);
                prop_assert!(pad.iter().all(|byte| *byte == filler), "row padding was overwritten");
                for (x, bytes) in pixels.chunks_exact(4).enumerate() {
                    let expected = image.get_pixel(u32::try_from(x).unwrap_or(0), u32::try_from(y).unwrap_or(0)).0;
                    prop_assert_eq!(pixel(format, [bytes[0], bytes[1], bytes[2], bytes[3]]), expected);
                }
            }
        }

        #[test]
        fn mismatched_buffers_are_refused(image in generated_image(), short in 1_usize..4) {
            let (width, height) = image.dimensions();
            let row = usize::try_from(width).unwrap_or(0).saturating_mul(4);
            let mut buffer = vec![0; row.saturating_mul(usize::try_from(height).unwrap_or(0))];
            prop_assert_eq!(
                encode(&image, ServerFormat::Argb8888, (width.saturating_add(1), height), row, &mut buffer),
                Err(BufferMismatch::Size)
            );
            prop_assert_eq!(
                encode(&image, ServerFormat::Rgb565, (width, height), row, &mut buffer),
                Err(BufferMismatch::Format)
            );
            prop_assert_eq!(
                encode(&image, ServerFormat::Argb8888, (width, height), row.saturating_sub(1), &mut buffer),
                Err(BufferMismatch::Stride)
            );
            let cut = buffer.len().saturating_sub(short);
            prop_assert_eq!(
                encode(&image, ServerFormat::Argb8888, (width, height), row, &mut buffer[..cut]),
                Err(BufferMismatch::Length)
            );
        }
    }

    #[derive(Debug, Default)]
    struct App {
        size: Option<(u32, u32)>,
        formats: Vec<wl_shm::Format>,
        done: bool,
        ready: bool,
        failed: Option<u32>,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for App {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as wayland_client::Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ExtImageCopyCaptureSessionV1,
            event: ext_image_copy_capture_session_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                    app.size = Some((width, height));
                }
                ext_image_copy_capture_session_v1::Event::ShmFormat {
                    format: wayland_client::WEnum::Value(format),
                } => app.formats.push(format),
                ext_image_copy_capture_session_v1::Event::Done => app.done = true,
                _ => {}
            }
        }
    }
    impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for App {
        fn event(
            app: &mut Self,
            _: &ExtImageCopyCaptureFrameV1,
            event: ext_image_copy_capture_frame_v1::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                ext_image_copy_capture_frame_v1::Event::Ready => app.ready = true,
                ext_image_copy_capture_frame_v1::Event::Failed { reason } => app.failed = Some(reason.into()),
                _ => {}
            }
        }
    }
    delegate_noop!(App: ignore WlOutput);
    delegate_noop!(App: ignore WlShm);
    delegate_noop!(App: ignore WlShmPool);
    delegate_noop!(App: ignore WlBuffer);
    delegate_noop!(App: ExtOutputImageCaptureSourceManagerV1);
    delegate_noop!(App: ExtImageCaptureSourceV1);
    delegate_noop!(App: ExtImageCopyCaptureManagerV1);

    /// What a capturing client saw: buffer size, formats, and the first pixel of its buffer.
    #[derive(Debug)]
    struct Report {
        size: Option<(u32, u32)>,
        formats: Vec<wl_shm::Format>,
        ready: bool,
        failed: Option<u32>,
        first_pixel: [u8; 4],
    }

    fn client(stream: UnixStream) -> Report {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let output: WlOutput = globals.bind(&handle, 1..=4, ()).unwrap();
        let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
        let sources: ExtOutputImageCaptureSourceManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
        let manager: ExtImageCopyCaptureManagerV1 = globals.bind(&handle, 1..=1, ()).unwrap();
        let source = sources.create_source(&output, &handle, ());
        let session = manager.create_session(&source, Options::empty(), &handle, ());
        let mut app = App::default();
        while !app.done {
            let _dispatched = queue.roundtrip(&mut app).unwrap();
        }
        let (width, height) = app.size.unwrap();
        let stride = i32::try_from(width.saturating_mul(4)).unwrap();
        let bytes = stride.saturating_mul(i32::try_from(height).unwrap());
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0; usize::try_from(bytes).unwrap()]).unwrap();
        let pool = shm.create_pool(file.as_fd(), bytes, &handle, ());
        let buffer = pool.create_buffer(
            0,
            i32::try_from(width).unwrap(),
            i32::try_from(height).unwrap(),
            stride,
            wl_shm::Format::Argb8888,
            &handle,
            (),
        );
        let frame = session.create_frame(&handle, ());
        frame.attach_buffer(&buffer);
        frame.damage_buffer(
            0,
            0,
            i32::try_from(width).unwrap(),
            i32::try_from(height).unwrap(),
        );
        frame.capture();
        while !app.ready && app.failed.is_none() {
            let _dispatched = queue.roundtrip(&mut app).unwrap();
        }
        let mut first_pixel = [0; 4];
        let _start = file.seek(SeekFrom::Start(0)).unwrap();
        file.read_exact(&mut first_pixel).unwrap();
        Report {
            size: app.size,
            formats: app.formats,
            ready: app.ready,
            failed: app.failed,
            first_pixel,
        }
    }

    #[test]
    fn client_captures_the_headless_output_into_an_shm_buffer() {
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let colour = Rc::new(RefCell::new(image::Rgba([10, 20, 30, 255])));
        compositor.set_capture_source(&colour, |colour, _, _| {
            Ok(image::RgbaImage::from_pixel(1280, 720, *colour))
        });
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let thread = thread::spawn(move || client(stream));
        let started = Instant::now();
        while !thread.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(20), "client timed out");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        let report = thread.join().unwrap();
        assert_eq!(report.size, Some((1280, 720)), "buffer size is the output mode");
        assert!(report.formats.contains(&wl_shm::Format::Argb8888), "{report:?}");
        assert!(report.ready, "frame was not ready: {report:?}");
        assert_eq!(report.failed, None, "frame failed");
        assert_eq!(
            report.first_pixel,
            [30, 20, 10, 255],
            "Argb8888 stores blue first"
        );
    }
}
