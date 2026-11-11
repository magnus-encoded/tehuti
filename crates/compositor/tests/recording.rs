#[cfg(test)]
mod recording_client;
#[cfg(test)]
mod recording {
    use core::{cell::RefCell, time::Duration};
    use smithay::reexports::wayland_server::Display;
    use std::{io::Write as _, os::unix::fs::PermissionsExt, path::Path, rc::Rc, time::Instant};
    use tehuti::state::Compositor;
    use tehuti_protocol::{Event, Text};

    #[derive(Debug)]
    struct Runtime {
        clock: Rc<core::cell::Cell<Duration>>,
        mode: String,
        written: Rc<core::cell::Cell<usize>>,
        timeline: Option<TimelineFake>,
    }

    /// A timeline disk that takes at most 64 bytes per write, or fails every write.
    #[derive(Debug, Clone)]
    struct TimelineFake {
        bytes: Rc<RefCell<Vec<u8>>>,
        fail: bool,
    }

    impl std::io::Write for TimelineFake {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.fail {
                return Err(std::io::Error::other("injected timeline disk failure"));
            }
            let taken = bytes.len().min(64);
            self.bytes.borrow_mut().extend_from_slice(&bytes[..taken]);
            Ok(taken)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Log output of the probe, to count recorder warnings.
    #[derive(Debug, Clone, Default)]
    struct Logs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("log lock poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Logs {
        fn lines_with(&self, text: &str) -> usize {
            String::from_utf8_lossy(&self.0.lock().unwrap())
                .lines()
                .filter(|line| line.contains(text))
                .count()
        }
    }

    impl tehuti::recording_encoder::Runtime for Runtime {
        fn now(&self) -> Duration {
            self.clock.get()
        }
        fn timeline(&self, path: &Path) -> std::io::Result<Box<dyn std::io::Write>> {
            match &self.timeline {
                Some(fake) => Ok(Box::new(fake.clone())),
                None => Ok(Box::new(std::fs::File::create_new(path)?)),
            }
        }
        fn audio_sources(
            &self,
            microphone: &tehuti_core::dispatch::MicrophoneChoice,
        ) -> std::io::Result<Vec<String>> {
            if self.mode == "missing" {
                return Err(std::io::Error::other(
                    "ffmpeg missing: install the ffmpeg package",
                ));
            }
            let microphones = match microphone {
                tehuti_core::dispatch::MicrophoneChoice::Named(name) => vec![name.as_str()],
                tehuti_core::dispatch::MicrophoneChoice::Auto => vec!["mic-a"],
                tehuti_core::dispatch::MicrophoneChoice::All => vec!["mic-a", "mic-b"],
            };
            Ok(microphones
                .into_iter()
                .chain(["speakers.monitor"])
                .map(str::to_owned)
                .collect())
        }
        fn spawn(
            &self,
            command: &std::process::Command,
        ) -> std::io::Result<Box<dyn tehuti::recording_encoder::Sink>> {
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            if matches!(self.mode.as_str(), "live" | "backlog") {
                assert!(
                    args.windows(2)
                        .any(|args| args[0] == "-shortest_buf_duration" && args[1] == "1")
                );
                assert!(
                    args.windows(2)
                        .any(|args| { args[0] == "-movflags" && args[1] == "+frag_keyframe+empty_moov" })
                );
                assert!(
                    args.windows(2)
                        .any(|args| args[0] == "-max_interleave_delta" && args[1] == "1000000")
                );
            }
            let values = |flag: &str| {
                args.windows(2)
                    .filter(|pair| pair[0] == flag)
                    .map(|pair| pair[1].clone())
                    .collect::<Vec<_>>()
            };
            let video = serde_json::json!({
                "size": values("-video_size")[0], "fps": values("-framerate")[0],
                "inputs": values("-i"),
                "titles": args.iter().filter(|arg| arg.starts_with("title=")).collect::<Vec<_>>(),
                "dispositions": args.windows(2).filter(|pair| pair[0].starts_with("-disposition:a:")).map(|pair| &pair[1]).collect::<Vec<_>>(),
            });
            Ok(Box::new(Sink {
                path: args.last().unwrap().into(),
                video,
                fail: self.mode.starts_with("encoder_failure"),
                blocked: if self.mode == "flood" { 30 } else { 0 },
                written: Rc::clone(&self.written),
                frame_size: values("-video_size")[0]
                    .split_once('x')
                    .map(|(width, height)| {
                        width
                            .parse::<usize>()
                            .unwrap()
                            .checked_mul(height.parse::<usize>().unwrap())
                            .and_then(|pixels| pixels.checked_mul(4))
                            .unwrap()
                    })
                    .unwrap(),
            }))
        }
    }

    #[derive(Debug)]
    struct Sink {
        path: std::path::PathBuf,
        video: serde_json::Value,
        fail: bool,
        blocked: u32,
        written: Rc<core::cell::Cell<usize>>,
        frame_size: usize,
    }
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.fail {
                return Err(std::io::Error::other("injected encoder failure"));
            }
            if self.blocked > 0 {
                self.blocked = self.blocked.saturating_sub(1);
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            if self.video.get("pixel").is_none() {
                self.video["pixel"] = serde_json::json!(&bytes[..4]);
            }
            self.written.set(self.written.get().saturating_add(bytes.len()));
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl tehuti::recording_encoder::Sink for Sink {
        fn finish(&mut self) -> std::io::Result<()> {
            self.video["frames"] =
                serde_json::json!(self.written.get().checked_div(self.frame_size).unwrap());
            std::fs::write(&self.path, serde_json::to_vec(&self.video)?)
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one session checks video, timeline and failure recovery through public controls"
    )]
    fn voice_recording_writes_shared_capture_and_a_searchable_timeline() {
        if std::env::var_os("RECORDING_PROBE").is_none() {
            for mode in ["capture", "named", "auto", "missing", "encoder_failure"] {
                isolated(mode);
            }
            return;
        }
        let mode = std::env::var("RECORDING_PROBE").unwrap();
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        compositor.load_input_choices(tehuti::input_choice_store::Store::from_env().unwrap());
        let colour = Rc::new(RefCell::new(image::Rgba([240, 20, 60, 255])));
        let (width, height) = if matches!(
            mode.as_str(),
            "duration" | "orientation" | "streaming" | "live" | "backlog" | "housekeeping"
        ) {
            (64, 64)
        } else {
            (2560, 1440)
        };
        compositor.set_capture_source(&colour, move |colour, _, _| {
            if colour[0] == 0 {
                return Err(std::io::Error::other("capture temporarily unavailable").into());
            }
            let mut frame = image::RgbaImage::from_pixel(width, height, *colour);
            for y in 0..16 {
                for x in 0..16 {
                    frame.put_pixel(x, y, image::Rgba([20, 240, 40, 255]));
                }
            }
            Ok(frame)
        });
        let clock = Rc::new(core::cell::Cell::new(Duration::ZERO));
        let written = Rc::new(core::cell::Cell::new(0));
        let real_time = matches!(mode.as_str(), "duration" | "orientation" | "streaming");
        if !real_time {
            compositor.set_recording_runtime(Rc::new(Runtime {
                clock: Rc::clone(&clock),
                mode: mode.clone(),
                written: Rc::clone(&written),
                timeline: None,
            }));
        }
        compositor.say(Text::new("start recording".into()).unwrap());
        if mode == "missing" {
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .contains("ffmpeg package")
            );
            assert!(!compositor.quit_requested());
            return;
        }
        if mode == "capture" {
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == "REC")
            );
            // Repeated starts leave one encoder and one recording folder.
            compositor.say(Text::new("start recording".into()).unwrap());
        }
        compositor.handle(&Event::Focus {
            app_id: Some("org.test.Player".into()),
            title: "A voice controlled game".into(),
        });
        compositor.say(Text::new("an unmapped phrase".into()).unwrap());
        compositor.host_key(smithay::input::keyboard::Keycode::new(38), true);
        if real_time {
            let ready_deadline = Instant::now().checked_add(Duration::from_secs(8)).unwrap();
            while !home.join("encoder-ready").exists() && Instant::now() < ready_deadline {
                compositor.advance_time();
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(home.join("encoder-ready").exists());
        } else {
            clock.set(Duration::from_millis(100));
            compositor.advance_time();
        }
        if mode == "live" {
            assert!(written.get() > 0, "video frames must reach ffmpeg before stop");
            assert!(
                !home
                    .join("Videos/Recordings")
                    .read_dir()
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path()
                    .join("video.mp4")
                    .exists()
            );
        }
        if mode == "backlog" {
            clock.set(Duration::from_secs(24));
            compositor.advance_time();
        }
        if mode == "housekeeping" {
            for step in 1..=20 {
                clock.set(Duration::from_millis(step * 100));
                compositor.advance_time();
            }
        }
        if mode == "capture_failure" {
            *colour.borrow_mut() = image::Rgba([0, 0, 0, 255]);
            for _ in 0..5 {
                clock.set(clock.get() + Duration::from_millis(100));
                compositor.advance_time();
            }
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == "REC")
            );
            *colour.borrow_mut() = image::Rgba([240, 20, 60, 255]);
        }
        if mode == "flood" {
            for _ in 0..2048 {
                compositor.host_key(smithay::input::keyboard::Keycode::new(38), true);
            }
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == "REC"),
                "timeline pressure must leave recording active"
            );
        }
        if mode == "duration" {
            let until = Instant::now().checked_add(Duration::from_secs(7)).unwrap();
            while Instant::now() < until {
                compositor.advance_time();
                assert!(
                    compositor
                        .voice_bar()
                        .segments()
                        .iter()
                        .any(|segment| segment.text == "REC"),
                    "a mid-recording encoder stall must leave recording active"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(home.join("encoder-released").exists());
        }
        if mode == "streaming" {
            let folder = home
                .join("Videos/Recordings")
                .read_dir()
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let until = Instant::now().checked_add(Duration::from_secs(4)).unwrap();
            while Instant::now() < until {
                compositor.advance_time();
                std::thread::sleep(Duration::from_millis(30));
            }
            assert!(
                compositor
                    .voice_bar()
                    .segments()
                    .iter()
                    .any(|segment| segment.text == "REC"),
                "recording must still be active when video is checked"
            );
            assert_top_left_marker(&folder);
        }
        if !mode.starts_with("encoder_failure") {
            compositor.say(Text::new("stop recording".into()).unwrap());
        }
        if real_time {
            let saved_deadline = Instant::now().checked_add(Duration::from_secs(8)).unwrap();
            while !compositor.visible_notice_text().is_some_and(|text| {
                text.starts_with("RECORDING SAVED:") || text.starts_with("RECORDING FAILED:")
            }) && Instant::now() < saved_deadline
            {
                compositor.advance_time();
                std::thread::sleep(Duration::from_millis(10));
            }
        } else {
            for _ in 0..800 {
                compositor.advance_time();
                if compositor.visible_notice_text().is_some_and(|text| {
                    text.starts_with("RECORDING SAVED:") || text.starts_with("RECORDING FAILED:")
                }) {
                    break;
                }
            }
        }
        assert!(
            !compositor
                .voice_bar()
                .segments()
                .iter()
                .any(|segment| segment.text == "REC")
        );
        if mode == "streaming" {
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .contains("DESKTOP TRACK SILENT"),
                "{:?}",
                compositor.visible_notice_text()
            );
        }
        if mode.starts_with("encoder_failure") {
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .starts_with("RECORDING FAILED:")
            );
            clock.set(clock.get() + Duration::from_secs(4));
            compositor.handle(&Event::Tick {
                elapsed: Duration::from_secs(4),
            });
            compositor.advance_time();
            assert!(
                compositor
                    .visible_notice_text()
                    .unwrap()
                    .starts_with("RECORDING FAILED:")
            );
            if mode == "encoder_failure_east" {
                compositor.handle(&Event::Button {
                    name: tehuti_protocol::ButtonName::East,
                    state: tehuti_protocol::ButtonState::Pressed,
                });
            } else {
                compositor.host_key(smithay::input::keyboard::Keycode::new(9), true);
            }
            assert!(
                !compositor
                    .visible_notice_text()
                    .is_some_and(|text| text.starts_with("RECORDING FAILED:"))
            );
            assert!(!compositor.quit_requested());
            return;
        }
        assert!(
            compositor
                .visible_notice_text()
                .unwrap()
                .starts_with("RECORDING SAVED:"),
            "notice: {:?}",
            compositor.visible_notice_text()
        );
        let folders: Vec<_> = std::fs::read_dir(home.join("Videos/Recordings"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(folders.len(), 1);
        let folder = &folders[0];
        if matches!(mode.as_str(), "orientation" | "streaming") {
            assert_top_left_marker(folder);
            return;
        }
        if mode == "duration" {
            assert!(home.join("encoder-released").exists());
            assert_wall_duration(folder);
            return;
        }
        let video: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("video.mp4")).unwrap()).unwrap();
        assert_eq!(video["pixel"], serde_json::json!([20, 240, 40, 255]));
        assert_eq!(video["size"], if width == 64 { "64x64" } else { "1920x1080" });
        assert_eq!(video["fps"], "30");
        if mode == "backlog" {
            assert_eq!(
                video["frames"], 720,
                "the fake ffmpeg must finalise every due frame"
            );
        }
        let microphones = match mode.as_str() {
            "named" => vec!["mic-b"],
            "auto" => vec!["mic-a"],
            _ => vec!["mic-a", "mic-b"],
        };
        let mut inputs = vec!["pipe:0"];
        inputs.extend(microphones.iter().copied());
        inputs.push("speakers.monitor");
        assert_eq!(video["inputs"], serde_json::json!(inputs));
        let titles: Vec<_> = microphones
            .iter()
            .chain(core::iter::once(&"speakers.monitor"))
            .map(|source| format!("title={source}"))
            .collect();
        assert_eq!(video["titles"], serde_json::json!(titles));
        assert_eq!(video["dispositions"][0], "default");
        assert!(
            video["dispositions"]
                .as_array()
                .unwrap()
                .iter()
                .skip(1)
                .all(|value| value == "0")
        );
        let text = std::fs::read_to_string(folder.join("timeline.jsonl")).unwrap();
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(events.iter().any(|event| event["kind"] == "screen"
            && event["data"]["bar"].is_array()
            && event["data"]["notices"].is_array()
            && event["data"]["windows"].is_array()));
        assert!(events.iter().any(|event| event["kind"] == "stop"));
        assert!(events.iter().any(|event| event["kind"] == "key"));
        if mode == "housekeeping" {
            for housekeeping in ["PollSpeech", "PollSources", "Supervise"] {
                assert!(
                    !text.contains(housekeeping),
                    "{housekeeping} must stay out of the timeline"
                );
            }
        }
        assert!(text.contains("an unmapped phrase"));
        assert!(text.contains("SpeechRouted"));
        assert!(text.contains("org.test.Player"));
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0]["time_ms"].as_u64() <= pair[1]["time_ms"].as_u64())
        );
    }

    type EncodedFrames = Rc<RefCell<Vec<(Duration, Vec<u8>)>>>;

    #[derive(Debug)]
    struct PixelRuntime {
        clock: Rc<core::cell::Cell<Duration>>,
        pixels: EncodedFrames,
    }

    impl tehuti::recording_encoder::Runtime for PixelRuntime {
        fn now(&self) -> Duration {
            self.clock.get()
        }
        fn audio_sources(&self, _: &tehuti_core::dispatch::MicrophoneChoice) -> std::io::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn spawn(
            &self,
            command: &std::process::Command,
        ) -> std::io::Result<Box<dyn tehuti::recording_encoder::Sink>> {
            let args: Vec<_> = command.get_args().map(|arg| arg.to_string_lossy()).collect();
            let size = args.windows(2).find(|pair| pair[0] == "-video_size").unwrap()[1]
                .split_once('x')
                .unwrap();
            Ok(Box::new(PixelSink {
                pixels: Rc::clone(&self.pixels),
                clock: Rc::clone(&self.clock),
                pending: Vec::new(),
                size: size
                    .0
                    .parse::<usize>()
                    .map_err(std::io::Error::other)?
                    .checked_mul(size.1.parse::<usize>().map_err(std::io::Error::other)?)
                    .and_then(|pixels| pixels.checked_mul(4))
                    .ok_or_else(|| std::io::Error::other("frame size overflow"))?,
            }))
        }
    }

    #[derive(Debug)]
    struct PixelSink {
        clock: Rc<core::cell::Cell<Duration>>,
        pixels: EncodedFrames,
        pending: Vec<u8>,
        size: usize,
    }
    impl std::io::Write for PixelSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let count = bytes.len().min(self.size / 2);
            self.pending.extend_from_slice(&bytes[..count]);
            if self.pending.len() == self.size {
                self.pixels
                    .borrow_mut()
                    .push((self.clock.get(), core::mem::take(&mut self.pending)));
            }
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl tehuti::recording_encoder::Sink for PixelSink {
        fn finish(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn changing_tiled_client_reaches_encoder_under_backpressure() {
        changing_client(
            "tiled",
            "recording::changing_tiled_client_reaches_encoder_under_backpressure",
        );
    }

    #[test]
    fn changing_fullscreen_client_reaches_encoder_under_backpressure() {
        changing_client(
            "fullscreen",
            "recording::changing_fullscreen_client_reaches_encoder_under_backpressure",
        );
    }

    fn assert_fresh_capture(compositor: &mut Compositor) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fresh.png");
        let mut driver =
            tehuti::driver::Driver::bind(directory.path(), tehuti::backend::Backend::Nested).unwrap();
        let _shot = driver
            .execute(compositor, tehuti::driver::Request::Shot { path: path.clone() })
            .unwrap();
        assert!(
            image::open(path)
                .unwrap()
                .to_rgba8()
                .pixels()
                .any(|pixel| pixel.0[1] > 80 && pixel.0[0] < 10 && pixel.0[2] < 10),
            "the shared capture path already contains the new client pixels"
        );
    }

    fn changing_client(mode: &str, test: &str) {
        if std::env::var_os("RECORDING_PROBE").is_none() {
            isolated_in(test, mode);
            return;
        }
        let mut display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let client = super::recording_client::Client::new(&display, mode == "fullscreen");
        client.paint(&mut display, &mut compositor, [0, 0, 255, 255]);
        let clock = Rc::new(core::cell::Cell::new(Duration::ZERO));
        let pixels = Rc::new(RefCell::new(Vec::new()));
        compositor.set_recording_runtime(Rc::new(PixelRuntime {
            clock: Rc::clone(&clock),
            pixels: Rc::clone(&pixels),
        }));
        compositor.say(Text::new("start recording".into()).unwrap());
        for step in 1..=90_u64 {
            if step == 60 {
                client.paint(&mut display, &mut compositor, [0, 255, 0, 255]);
                assert_fresh_capture(&mut compositor);
            }
            clock.set(Duration::from_millis(step.saturating_mul(34)));
            compositor.advance_time();
        }
        let frames = pixels.borrow();
        assert!(
            frames
                .first()
                .unwrap()
                .1
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] > 80 && pixel[1] < 10 && pixel[2] < 10),
            "initial client is red"
        );
        assert!(
            frames
                .last()
                .unwrap()
                .1
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[1] > 80 && pixel[0] < 10 && pixel[2] < 10),
            "changing {mode} client must reach ffmpeg while recording"
        );
        drop(frames);
        compositor.say(Text::new("stop recording".into()).unwrap());
        for _ in 0..200 {
            compositor.advance_time();
        }
        client.finish();
    }

    #[test]
    fn slow_writer_receives_a_frame_no_older_than_one_interval_plus_write_time() {
        if std::env::var_os("RECORDING_PROBE").is_none() {
            isolated_in(
                "recording::slow_writer_receives_a_frame_no_older_than_one_interval_plus_write_time",
                "freshness",
            );
            return;
        }
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let clock = Rc::new(core::cell::Cell::new(Duration::ZERO));
        let pixels = Rc::new(RefCell::new(Vec::new()));
        let source = Rc::new(RefCell::new(Rc::clone(&clock)));
        compositor.set_capture_source(&source, |capture_clock, _, _| {
            let stamp = u8::try_from(capture_clock.get().as_millis() / 34).unwrap();
            Ok(image::RgbaImage::from_pixel(
                2,
                2,
                image::Rgba([stamp, 0, 0, 255]),
            ))
        });
        compositor.set_recording_runtime(Rc::new(PixelRuntime {
            clock: Rc::clone(&clock),
            pixels: Rc::clone(&pixels),
        }));
        compositor.say(Text::new("start recording".into()).unwrap());
        for step in 1..=90_u64 {
            clock.set(Duration::from_millis(step.saturating_mul(34)));
            compositor.advance_time();
        }
        for (completed, bytes) in pixels.borrow().iter().skip(2) {
            let captured = Duration::from_millis(u64::from(bytes[0]) * 34);
            assert!(
                completed.saturating_sub(captured) <= Duration::from_millis(102),
                "completed at {completed:?}, but captured at {captured:?}"
            );
            assert!(
                bytes.as_chunks::<4>().0.iter().all(|pixel| pixel == &bytes[..4]),
                "partial frames must not tear"
            );
        }
        compositor.say(Text::new("stop recording".into()).unwrap());
        for _ in 0..200 {
            compositor.advance_time();
        }
        assert_eq!(
            pixels.borrow().len(),
            91,
            "video duration follows the clock despite dropped captures"
        );
    }

    #[test]
    fn east_button_dismisses_recording_encoder_failure() {
        isolated("encoder_failure_east");
    }

    #[test]
    fn recording_duration_matches_wall_time_with_a_delayed_encoder() {
        use std::io::{self, Write};
        use tehuti::recording_encoder::FrameEncoder;

        struct FakeEncoder {
            elapsed: Rc<core::cell::Cell<Duration>>,
            bytes: Vec<u8>,
            stalled: bool,
        }
        impl Write for FakeEncoder {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.bytes.len() == 120 && !self.stalled {
                    self.elapsed.set(Duration::from_millis(4200));
                    self.stalled = true;
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let count = bytes.len().min(3);
                self.bytes.extend_from_slice(&bytes[..count]);
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let elapsed = Rc::new(core::cell::Cell::new(Duration::from_secs(1)));
        let mut writer = FakeEncoder {
            elapsed: Rc::clone(&elapsed),
            bytes: Vec::new(),
            stalled: false,
        };
        let mut encoder = FrameEncoder::default();
        for (time, frames) in [(1, 30), (2, 126), (7, 210)] {
            elapsed.set(Duration::from_secs(time));
            let mut caught_up = false;
            for _ in 0..2000 {
                if encoder
                    .write_due(&mut writer, &[20, 240, 40, 255], || elapsed.get())
                    .unwrap()
                {
                    caught_up = true;
                    break;
                }
            }
            assert!(
                caught_up,
                "encoder must catch up after a stall longer than two seconds"
            );
            assert_eq!(
                writer.bytes.len(),
                frames * 4,
                "frames must match elapsed fake time at 30 fps"
            );
            assert!(
                writer
                    .bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| *pixel == [20, 240, 40, 255])
            );
        }
    }

    #[test]
    #[ignore = "needs ffmpeg and real time; run with --ignored"]
    fn real_ffmpeg_preserves_duration_audio_and_orientation() {
        isolated("duration");
        isolated("orientation");
    }

    #[test]
    fn capture_failure_keeps_recording_active() {
        isolated("capture_failure");
    }

    #[test]
    fn timeline_pressure_keeps_recording_active() {
        isolated("flood");
    }

    #[test]
    fn frames_reach_ffmpeg_during_recording() {
        isolated("live");
    }

    #[test]
    fn real_ffmpeg_streams_playable_frames_before_stop() {
        isolated("streaming");
    }

    fn playable_fragment(path: &Path) {
        let output = std::process::Command::new("/usr/bin/ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=64x64:r=30",
                "-t",
                "1",
                "-c:v",
                "libx264",
                "-movflags",
                "+frag_keyframe+empty_moov",
                "-y",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn assert_playable(path: &Path) {
        let output = std::process::Command::new("/usr/bin/ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("codec_name=h264"));
    }

    #[test]
    fn progressing_ffmpeg_is_not_killed_after_thirty_virtual_seconds() {
        let folder = tempfile::tempdir().unwrap();
        let video = folder.path().join("video.mp4");
        playable_fragment(&video);
        let mut child = std::process::Command::new("/usr/bin/python3")
            .args(["-c", "import time; time.sleep(0.1)"])
            .spawn()
            .unwrap();
        let mut elapsed = Duration::ZERO;
        let result = tehuti::recording_encoder::finish_process_with_clock(
            &mut child,
            &video,
            Ok(()),
            || {
                elapsed += Duration::from_secs(10);
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&video)
                    .unwrap()
                    .write_all(&[0])
                    .unwrap();
                elapsed
            },
            || std::thread::sleep(Duration::from_millis(5)),
        );
        assert!(result.is_ok(), "progressing encoder was killed: {result:?}");
        assert!(elapsed > Duration::from_secs(30));
        assert_playable(&video);
    }

    #[test]
    fn stalled_ffmpeg_is_killed_after_thirty_virtual_seconds_and_fragment_stays_playable() {
        let folder = tempfile::tempdir().unwrap();
        let video = folder.path().join("video.mp4");
        playable_fragment(&video);
        let mut child = std::process::Command::new("/usr/bin/python3")
            .args(["-c", "import time; time.sleep(60)"])
            .spawn()
            .unwrap();
        let mut elapsed = Duration::ZERO;
        let result = tehuti::recording_encoder::finish_process_with_clock(
            &mut child,
            &video,
            Ok(()),
            || {
                elapsed += Duration::from_secs(10);
                elapsed
            },
            || {},
        );
        assert!(result.unwrap_err().to_string().contains("stopped finalising"));
        assert!(elapsed >= Duration::from_secs(30));
        assert!(
            child.try_wait().unwrap().is_some(),
            "stalled encoder must be reaped"
        );
        assert_playable(&video);
    }

    #[test]
    fn stopped_recording_with_backlog_finalises_video() {
        isolated("backlog");
    }

    #[test]
    fn timeline_omits_housekeeping_effects() {
        isolated("housekeeping");
    }

    fn assert_top_left_marker(folder: &Path) {
        let output = std::process::Command::new("/usr/bin/ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(folder.join("video.mp4"))
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let frame = image::RgbaImage::from_raw(64, 64, output.stdout).unwrap();
        let marker = frame.get_pixel(8, 8);
        assert!(
            marker[1] > 200 && marker[0] < 60 && marker[2] < 80,
            "encoded first frame must keep the green marker top-left: {marker:?}"
        );
        for (x, y) in [(55, 8), (8, 55), (55, 55)] {
            assert!(frame.get_pixel(x, y)[0] > 200, "marker moved to another corner");
        }
    }

    fn assert_wall_duration(folder: &Path) {
        let timeline = std::fs::read_to_string(folder.join("timeline.jsonl")).unwrap();
        let elapsed_ms = timeline
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|event| event["kind"] == "stop")
            .expect("timeline must contain the explicit stop")["time_ms"]
            .as_u64()
            .unwrap();
        let output = std::process::Command::new("/usr/bin/ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,duration,start_time:stream_tags=handler_name:stream_disposition=default",
                "-of",
                "json",
            ])
            .arg(folder.join("video.mp4"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let probe: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let streams = probe["streams"].as_array().unwrap();
        assert_eq!(
            streams.len(),
            4,
            "video, both microphones and desktop audio must survive"
        );
        let audio: Vec<_> = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .collect();
        for (stream, source) in audio.iter().zip(["mic-a", "mic-b", "speakers.monitor"]) {
            assert_eq!(stream["tags"]["handler_name"], source);
        }
        assert_eq!(audio[0]["disposition"]["default"], 1);
        assert!(
            audio
                .iter()
                .skip(1)
                .all(|stream| stream["disposition"]["default"] == 0)
        );
        let elapsed = Duration::from_millis(elapsed_ms);
        assert!(elapsed >= Duration::from_millis(2900));
        for stream in streams {
            let duration = Duration::from_secs_f64(stream["duration"].as_str().unwrap().parse().unwrap());
            assert!(
                duration.abs_diff(elapsed) <= elapsed.div_f64(20.0),
                "video duration must match wall time within 5%, including audio: {duration:?} versus {elapsed:?}"
            );
            let start: f64 = stream["start_time"].as_str().unwrap().parse().unwrap();
            assert!(start.abs() <= 0.05, "tracks must share the session origin");
        }
    }

    fn executable(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn install_video_encoder(bin: &Path) {
        executable(
            &bin.join("ffmpeg"),
            r"#!/usr/bin/python3
import os, subprocess, sys, time
from pathlib import Path
args = sys.argv[1:]
home = Path(os.environ['HOME'])
# Model the pulse demuxer: lavfi silence stamped on the wall clock like `-wallclock 1`.
while '-wallclock' in args:
    del args[args.index('-wallclock'):args.index('-wallclock') + 2]
for index in range(len(args)):
    if args[index] == 'pulse':
        args[index] = 'lavfi'
    if args[index] in ('speakers.monitor', 'mic-a', 'mic-b'):
        args[index] = f'anullsrc=r=48000:cl=stereo,asetpts=PTS+{time.time():.6f}/TB'
child = subprocess.Popen(['/usr/bin/ffmpeg'] + args, stdin=subprocess.PIPE)
began = time.monotonic()
stalled = False
while True:
    frame = sys.stdin.buffer.read(64 * 64 * 4)
    if not frame:
        break
    child.stdin.write(frame)
    child.stdin.flush()
    home.joinpath('encoder-ready').touch()
    if os.environ['RECORDING_PROBE'] == 'duration' and not stalled and time.monotonic() - began >= 1:
        stalled = True
        time.sleep(3.2)
        home.joinpath('encoder-released').touch()
child.stdin.close()
sys.exit(child.wait())
",
        );
    }

    /// An encoder that reads one frame every 50 ms (20 fps, slower than the 30 fps clock) and
    /// logs, per frame, the wall time it arrived and the capture stamp in its first pixel.
    fn install_slow_encoder(bin: &Path) {
        executable(
            &bin.join("ffmpeg"),
            r"#!/usr/bin/python3
import os, sys, time
from pathlib import Path
log = open(Path(os.environ['HOME']) / 'frames.log', 'w')
while True:
    frame = sys.stdin.buffer.read(64 * 64 * 4)
    if len(frame) < 64 * 64 * 4:
        break
    stamp = int.from_bytes(frame[:4], 'little')
    log.write(f'{time.time()} {stamp}\n')
    log.flush()
    time.sleep(0.05)
Path(sys.argv[-1]).write_bytes(b'video')
",
        );
    }

    /// Drives the threaded encoder path: a writer slower than the clock still sends fresh
    /// captures before stop, and the file is finalised.
    #[test]
    fn a_slow_encoder_process_receives_fresh_frames_and_the_file_is_finalised() {
        if std::env::var_os("RECORDING_PROBE").is_none() {
            isolated_in(
                "recording::a_slow_encoder_process_receives_fresh_frames_and_the_file_is_finalised",
                "slow_process",
            );
            return;
        }
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        let started = Rc::new(RefCell::new(Instant::now()));
        compositor.set_capture_source(&started, |started, _, _| {
            let stamp = u32::try_from(started.elapsed().as_millis()).unwrap();
            Ok(image::RgbaImage::from_pixel(
                64,
                64,
                image::Rgba(stamp.to_le_bytes()),
            ))
        });
        compositor.say(Text::new("start recording".into()).unwrap());
        let stop_at = Instant::now().checked_add(Duration::from_secs(3)).unwrap();
        while Instant::now() < stop_at {
            compositor.advance_time();
            std::thread::sleep(Duration::from_millis(5));
        }
        let stopped = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let stopped_stamp = started.borrow().elapsed().as_millis();
        compositor.say(Text::new("stop recording".into()).unwrap());
        wait_until_saved(&mut compositor);
        let folder = home
            .join("Videos/Recordings")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(folder.join("video.mp4")).unwrap(), b"video");
        let newest_before_stop = newest_sent_before(&home.join("frames.log"), stopped);
        assert!(
            newest_before_stop.saturating_add(500) >= stopped_stamp,
            "newest capture sent before stop was {newest_before_stop} ms, stop at {stopped_stamp} ms"
        );
    }

    fn wait_until_saved(compositor: &mut Compositor) {
        let deadline = Instant::now().checked_add(Duration::from_secs(30)).unwrap();
        while compositor
            .visible_notice_text()
            .is_none_or(|text| !text.contains("RECORDING SAVED"))
            && Instant::now() < deadline
        {
            compositor.advance_time();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            compositor
                .visible_notice_text()
                .is_some_and(|text| text.contains("RECORDING SAVED")),
            "{:?}",
            compositor.visible_notice_text()
        );
    }

    /// The newest capture stamp the slow encoder received before wall time `stopped`.
    fn newest_sent_before(log: &Path, stopped: f64) -> u128 {
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(arrived, stamp)| (arrived.parse::<f64>().unwrap(), stamp.parse::<u128>().unwrap()))
            .filter(|(arrived, _)| *arrived < stopped)
            .map(|(_, stamp)| stamp)
            .max()
            .unwrap()
    }

    fn encoder_logs(home: &Path) -> String {
        let mut logs = String::new();
        if let Ok(folders) = std::fs::read_dir(home.join("Videos/Recordings")) {
            for folder in folders.flatten() {
                logs.push_str(&std::fs::read_to_string(folder.path().join("ffmpeg.log")).unwrap_or_default());
            }
        }
        logs
    }

    #[test]
    fn a_full_timeline_queue_logs_once_when_dropping_starts_and_once_with_the_count() {
        isolated_in("recording::timeline_probe", "flood");
    }

    #[test]
    fn ten_minutes_of_pad_motion_keep_the_timeline_current_without_ticks_or_repeats() {
        isolated_in("recording::timeline_probe", "pad");
    }

    #[test]
    fn two_identical_presses_100_ms_apart_are_both_written() {
        isolated_in("recording::timeline_probe", "presses");
    }

    #[test]
    fn a_failing_timeline_disk_logs_one_line_and_the_video_still_finalises() {
        isolated_in("recording::timeline_probe", "failing");
    }

    /// Runs one timeline law in an isolated process; `isolated_in` sets `RECORDING_PROBE`.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one recording session per law, driven through public controls"
    )]
    fn timeline_probe() {
        let Ok(mode) = std::env::var("RECORDING_PROBE") else {
            return;
        };
        let logs = Logs::default();
        let writer = logs.clone();
        let _logging = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish(),
        );
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        tehuti::driver::headless_output(&display.handle(), &mut compositor).unwrap();
        compositor.load_input_choices(tehuti::input_choice_store::Store::from_env().unwrap());
        let colour = Rc::new(RefCell::new(image::Rgba([240, 20, 60, 255])));
        compositor.set_capture_source(&colour, |colour, _, _| {
            Ok(image::RgbaImage::from_pixel(64, 64, *colour))
        });
        let clock = Rc::new(core::cell::Cell::new(Duration::ZERO));
        let written = Rc::new(core::cell::Cell::new(0));
        let bytes = Rc::new(RefCell::new(Vec::new()));
        compositor.set_recording_runtime(Rc::new(Runtime {
            clock: Rc::clone(&clock),
            mode: mode.clone(),
            written: Rc::clone(&written),
            timeline: Some(TimelineFake {
                bytes: Rc::clone(&bytes),
                fail: mode == "failing",
            }),
        }));
        compositor.say(Text::new("start recording".into()).unwrap());
        let advance = |target: &mut Compositor, moment: Duration| {
            clock.set(moment);
            target.advance_time();
        };
        match mode.as_str() {
            "flood" => {
                // Nothing drains the queue between these keys.
                for _ in 0..4096 {
                    compositor.host_key(smithay::input::keyboard::Keycode::new(38), true);
                }
                assert_eq!(
                    logs.lines_with("timeline queue full"),
                    1,
                    "dropping must log once, not per event"
                );
                advance(&mut compositor, Duration::from_millis(100));
            }
            "pad" => {
                for tenth in 1..=6000_u64 {
                    for sample in 0..10_i16 {
                        let value = i16::try_from(tenth % 1000).unwrap() * 30 + sample;
                        compositor.sample_sticks(tehuti_core::dispatch::Sticks {
                            left: (value, -value),
                            right: (0, value),
                        });
                        compositor.handle(&Event::Tick {
                            elapsed: Duration::from_micros(371),
                        });
                    }
                    advance(&mut compositor, Duration::from_millis(tenth * 100));
                }
            }
            "presses" => {
                let south = |disk: &RefCell<Vec<u8>>| {
                    String::from_utf8_lossy(&disk.borrow())
                        .lines()
                        .filter(|line| line.contains("South"))
                        .count()
                };
                let press = Event::Button {
                    name: tehuti_protocol::ButtonName::South,
                    state: tehuti_protocol::ButtonState::Pressed,
                };
                compositor.handle(&press);
                advance(&mut compositor, Duration::from_millis(100));
                let first = south(&bytes);
                compositor.handle(&press);
                advance(&mut compositor, Duration::from_millis(200));
                assert!(first > 0, "the first press is in the timeline");
                assert_eq!(south(&bytes), first * 2, "the second press is written too");
            }
            _ => {
                for tenth in 1..=30_u64 {
                    compositor.host_key(smithay::input::keyboard::Keycode::new(38), true);
                    advance(&mut compositor, Duration::from_millis(tenth * 100));
                }
            }
        }
        compositor.say(Text::new("stop recording".into()).unwrap());
        // The fake encoder takes one frame per pass; ten minutes leave a backlog to finalise.
        for _ in 0..40_000 {
            compositor.advance_time();
            if compositor.visible_notice_text().is_some_and(|text| {
                text.starts_with("RECORDING SAVED:") || text.starts_with("RECORDING FAILED:")
            }) {
                break;
            }
        }
        assert!(
            compositor
                .visible_notice_text()
                .is_some_and(|text| text.starts_with("RECORDING SAVED:")),
            "notice: {:?}",
            compositor.visible_notice_text()
        );
        let folder = std::fs::read_dir(
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("Videos/Recordings"),
        )
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
        let video: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("video.mp4")).unwrap()).unwrap();
        assert!(video["frames"].as_u64().unwrap() > 0, "the video must finalise");
        if mode == "failing" {
            assert_eq!(
                logs.lines_with("timeline"),
                1,
                "a failing timeline must end with exactly one line"
            );
            return;
        }
        let text = String::from_utf8(bytes.borrow().clone()).unwrap();
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let ending = events.last().unwrap();
        assert_eq!(ending["kind"], "stop");
        if mode == "flood" {
            assert_eq!(
                logs.lines_with("timeline queue"),
                2,
                "dropping ends with one line carrying the count"
            );
            assert!(
                String::from_utf8_lossy(&logs.0.lock().unwrap()).contains("count="),
                "the end line carries how many events were dropped"
            );
            return;
        }
        assert!(
            !text.contains("Tick {"),
            "loop ticks are not timeline events: {:?}",
            text.lines().find(|line| line.contains("Tick {"))
        );
        let sticks = text.matches("Sticks").count();
        assert!(sticks <= 6001, "{sticks} stick entries for 6000 video frames");
        let mut counts = std::collections::HashMap::new();
        for event in events
            .iter()
            .filter(|event| event["kind"] == "input" || event["kind"] == "effect")
        {
            *counts.entry(event["data"].to_string()).or_insert(0_u32) += 1;
        }
        assert!(
            counts.values().all(|count| *count <= 600),
            "a repeated identical event is written once: {:?}",
            counts.iter().max_by_key(|(_, count)| **count)
        );
        let last = events[events.len() - 2]["time_ms"].as_u64().unwrap();
        assert!(
            ending["time_ms"].as_u64().unwrap().saturating_sub(last) <= 1000,
            "last entry at {last} ms, video ends at {}",
            ending["time_ms"]
        );
        assert_eq!(
            logs.lines_with("queue"),
            0,
            "a pad in motion must not fill the queue"
        );
    }

    fn isolated(mode: &str) {
        isolated_in(
            "recording::voice_recording_writes_shared_capture_and_a_searchable_timeline",
            mode,
        );
    }

    fn isolated_in(test: &str, mode: &str) {
        if matches!(mode, "duration" | "orientation" | "streaming") {
            for program in ["/usr/bin/ffmpeg", "/usr/bin/ffprobe"] {
                assert!(Path::new(program).is_file(), "install ffmpeg: {program} missing");
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("pactl"),
            "#!/bin/sh\ncase \"$1\" in get-default-source) echo mic-a;; *) echo speakers;; esac\n",
        );
        let listing = serde_json::json!(["mic-a", "mic-b"].map(|name| serde_json::json!({
            "type": "PipeWire:Interface:Node", "info": {"props": {
                "node.name": name, "media.class": "Audio/Source"}}
        })));
        executable(
            &bin.join("pw-dump"),
            &format!("#!/bin/sh\nprintf '%s' '{listing}'\n"),
        );
        let config = directory.path().join("config/tehuti");
        std::fs::create_dir_all(&config).unwrap();
        let choice = match mode {
            "named" => "mic-b",
            "auto" => "auto",
            _ => "all",
        };
        std::fs::write(config.join("speech-source"), choice).unwrap();
        if matches!(mode, "duration" | "orientation" | "streaming") {
            install_video_encoder(&bin);
        }
        if mode == "slow_process" {
            install_slow_encoder(&bin);
        }
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test])
            .env("RECORDING_PROBE", mode)
            .env("HOME", directory.path())
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .env("PATH", bin)
            .env(
                "TEHUTI_TEST_SIZE",
                if matches!(mode, "tiled" | "fullscreen") {
                    "320x240"
                } else {
                    "1280x720"
                },
            )
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{mode}: {}\n{}{}\n{}",
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
            encoder_logs(directory.path())
        );
    }
}
