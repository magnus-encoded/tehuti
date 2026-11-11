//! Local video, separate `PulseAudio` tracks and a text timeline.
use crate::state::Compositor;
use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use image::RgbaImage;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, Read as _, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    time::Instant,
};

const INTERVAL: Duration = Duration::from_nanos(33_333_334);

/// Video writing trails the clock by one frame. `write_due` writes slot `s` once
/// `now - lag >= (s + 1) / 30`, and a frame for slot `s` is captured by `s / 30 + 17 ms`, so render
/// and queue get about 50 ms. A frame later than that is shown one slot late, not lost.
const CAPTURE_LAG: Duration = INTERVAL;

/// Timeline events the encoder loop writes before it turns back to video.
const EVENT_BATCH: usize = 256;

/// Recording time up to which video slots may be written.
const fn video_due(elapsed: Duration, stopped_ms: u64) -> Duration {
    if stopped_ms == u64::MAX {
        elapsed.saturating_sub(CAPTURE_LAG)
    } else {
        Duration::from_millis(stopped_ms)
    }
}

#[derive(Debug)]
pub(super) struct Session {
    frames: SyncSender<(Duration, RgbaImage)>,
    events: SyncSender<Value>,
    result: Receiver<Result<PathBuf, String>>,
    stop: Arc<AtomicU64>,
    started: Instant,
    /// `started` on the wall clock `PulseAudio` stamps audio with, since the Unix epoch.
    wall_start: Duration,
    next_frame: Instant,
    scene: Value,
    worker: Option<std::thread::JoinHandle<()>>,
    /// The timeline writer, joined on drop so the final `stop` line is on disk.
    timeline: Option<std::thread::JoinHandle<()>>,
    injected: Option<Injected>,
    /// Events dropped since the timeline queue last accepted one.
    dropped_events: Cell<u64>,
    /// Frames repeated since the capture queue last accepted one.
    repeated_frames: Cell<u64>,
    /// The newest stick sample, written at most once per video frame.
    sticks: RefCell<Option<String>>,
    /// The last stick sample written, so a resting stick is not written every frame.
    written_sticks: RefCell<String>,
}

#[derive(Debug)]
struct Injected {
    runtime: std::rc::Rc<dyn crate::recording_encoder::Runtime>,
    began: Duration,
    sink: Box<dyn crate::recording_encoder::Sink>,
    encoder: crate::recording_encoder::FrameEncoder,
    schedule: crate::recording_encoder::FrameSchedule<RgbaImage>,
    pixels: RgbaImage,
    timeline: Timeline<Box<dyn Write>>,
    events: Receiver<Value>,
    frames: Receiver<(Duration, RgbaImage)>,
    size: (u32, u32),
    result: SyncSender<Result<PathBuf, String>>,
    path: PathBuf,
}

impl Compositor {
    /// Supplies a recording clock and encoder for an embedded recording runtime.
    pub fn set_recording_runtime(&mut self, runtime: std::rc::Rc<dyn crate::recording_encoder::Runtime>) {
        self.use_driver_clock();
        self.recording_runtime = Some(runtime);
    }

    pub(super) fn recording_active(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|session| session.stop.load(Ordering::Relaxed) == u64::MAX)
    }

    pub(super) fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        match start(self) {
            Ok(session) => {
                self.recording = Some(session);
                self.record_event("start", &json!({"action": "start_recording"}));
                self.poll_recording();
            }
            Err(error) => self.show_recording_failure(&error.to_string()),
        }
    }

    pub(super) fn stop_recording(&self) {
        if let Some(session) = &self.recording {
            session.request_stop();
        }
    }

    pub(super) fn record_debug(&self, kind: &str, event: &impl core::fmt::Debug) {
        if !self.recording_active() {
            return;
        }
        self.record_event(kind, &json!({"event": format!("{event:?}")}));
    }

    /// Keeps the newest stick sample; `poll_recording` writes it once per video frame.
    pub(super) fn record_sticks(&self, sticks: &impl core::fmt::Debug) {
        if let Some(session) = &self.recording
            && session.stop.load(Ordering::Relaxed) == u64::MAX
        {
            *session.sticks.borrow_mut() = Some(format!("{sticks:?}"));
        }
    }

    fn flush_sticks(&self) {
        let Some(text) = self.recording.as_ref().and_then(|session| session.sticks.take()) else {
            return;
        };
        let changed = self.recording.as_ref().is_some_and(|session| {
            let mut written = session.written_sticks.borrow_mut();
            let changed = *written != text;
            if changed {
                written.clone_from(&text);
            }
            changed
        });
        if changed {
            self.record_event("input", &json!({"event": text}));
        }
    }

    pub(super) fn record_surface_change(&mut self) {
        if let Some(session) = &mut self.recording {
            session.scene = Value::Null;
        }
        self.record_screen();
    }

    pub(super) fn record_event(&self, kind: &str, data: &Value) {
        let Some(session) = &self.recording else { return };
        if session.stop.load(Ordering::Relaxed) != u64::MAX {
            return;
        }
        let event = json!({"time_ms": session.elapsed().as_millis(), "kind": kind, "data": data});
        match session.events.try_send(event) {
            Ok(()) => recovered(&session.dropped_events, "timeline queue drained; events dropped"),
            Err(TrySendError::Full(_)) => {
                dropping(
                    &session.dropped_events,
                    "recording timeline queue full; dropping events",
                );
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub(super) fn record_screen(&mut self) {
        if !self.recording_active() {
            return;
        }
        let mut scene = self.driver_scene();
        if let Some(object) = scene.as_object_mut() {
            let _clock = object.insert(
                "clock".into(),
                json!(chrono::Local::now().format("%H:%M").to_string()),
            );
            let _clock_ms = object.remove("clock_ms");
        }
        if self
            .recording
            .as_ref()
            .is_some_and(|session| session.scene != scene)
        {
            self.record_event("screen", &scene);
            if let Some(session) = &mut self.recording {
                session.scene = scene;
            }
        }
    }

    pub(super) fn poll_recording(&mut self) {
        if let Some(session) = &mut self.recording {
            session.pump();
        }
        let result = self
            .recording
            .as_ref()
            .and_then(|session| match session.result.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("recording worker disconnected".into())),
            });
        if let Some(result) = result {
            self.recording = None;
            match result {
                Ok(path) => self.show_notice(&format!("RECORDING SAVED: {}", path.display())),
                Err(error) => self.show_recording_failure(&error),
            }
            return;
        }
        if !self.recording_active() {
            return;
        }
        self.record_screen();
        let now = self.recording.as_ref().map_or_else(Instant::now, |session| {
            session
                .started
                .checked_add(session.elapsed())
                .unwrap_or(session.started)
        });
        if self
            .recording
            .as_ref()
            .is_none_or(|session| now < session.next_frame)
        {
            return;
        }
        if let Some(session) = &mut self.recording {
            session.next_frame = now.checked_add(INTERVAL).unwrap_or(now);
        }
        self.flush_sticks();
        let captured = self.recording.as_ref().map_or(Duration::ZERO, Session::elapsed);
        match crate::driver_shot::frame(self, &chrono::Local::now().format("%H:%M").to_string()) {
            Ok(frame) => {
                if let Some(session) = &self.recording {
                    match session.frames.try_send((captured, frame)) {
                        Ok(()) => {
                            recovered(&session.repeated_frames, "capture queue drained; frames repeated");
                        }
                        Err(TrySendError::Full(_)) => dropping(
                            &session.repeated_frames,
                            "recording capture queue full; repeating the previous frame",
                        ),
                        Err(TrySendError::Disconnected(_)) => {}
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "recording capture failed; repeating previous frame");
            }
        }
    }
}

/// Counts one dropped item; only the first of a run is logged.
fn dropping(counter: &Cell<u64>, message: &'static str) {
    let count = counter.get();
    if count == 0 {
        tracing::warn!("{message}");
    }
    counter.set(count.saturating_add(1));
}

/// Ends a run of dropped items with one line carrying its length.
fn recovered(counter: &Cell<u64>, message: &'static str) {
    let count = counter.replace(0);
    if count > 0 {
        tracing::warn!(count, "recording {message}");
    }
}

/// Buffered timeline output that stops, with one log line, at its first write error.
struct Timeline<W: Write> {
    out: Option<io::BufWriter<W>>,
}

impl<W: Write> core::fmt::Debug for Timeline<W> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Timeline")
            .field("open", &self.out.is_some())
            .finish()
    }
}

impl<W: Write> Timeline<W> {
    fn new(out: W) -> Self {
        Self {
            out: Some(io::BufWriter::new(out)),
        }
    }

    fn write(&mut self, event: &Value) {
        self.attempt(|out| {
            serde_json::to_writer(&mut *out, event)?;
            out.write_all(b"\n")
        });
    }

    fn flush(&mut self) {
        self.attempt(Write::flush);
    }

    fn finish(&mut self, stopped_ms: u64) {
        self.write(&json!({"time_ms": stopped_ms, "kind": "stop", "data": {}}));
        self.flush();
    }

    fn attempt(&mut self, write: impl FnOnce(&mut io::BufWriter<W>) -> io::Result<()>) {
        if let Some(out) = &mut self.out
            && let Err(error) = write(out)
        {
            tracing::warn!(%error, "recording timeline writer stopped; the video continues");
            if let Some(failed) = self.out.take() {
                // A failed writer must not be flushed again on drop.
                let (_inner, _unwritten) = failed.into_parts();
            }
        }
    }
}

/// Writes the timeline on its own thread, so a slow or stuck disk never holds up video.
///
/// The thread ends after the stop flag is set and the queue is empty, or when the session is gone;
/// either way the last line is `stop`, stamped with recording time.
fn spawn_timeline(
    out: impl Write + Send + 'static,
    events: Receiver<Value>,
    stop: Arc<AtomicU64>,
    started: Instant,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("tehuti-timeline".into())
        .spawn(move || {
            let mut timeline = Timeline::new(out);
            let stopped = || match stop.load(Ordering::Relaxed) {
                u64::MAX => u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX - 1),
                stopped => stopped,
            };
            loop {
                match events.recv_timeout(Duration::from_millis(50)) {
                    Ok(event) => timeline.write(&event),
                    Err(RecvTimeoutError::Timeout) => {
                        timeline.flush();
                        if stop.load(Ordering::Relaxed) != u64::MAX {
                            timeline.finish(stopped());
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        timeline.finish(stopped());
                        return;
                    }
                }
            }
        })
}

/// Takes at most one captured frame per encoder pass.
///
/// Draining until empty never ends when scaling a frame takes longer than the capture
/// interval: a new frame is always waiting, and video and timeline stall until stop.
fn take_frame(
    mut next: impl FnMut() -> Option<(Duration, RgbaImage)>,
    schedule: &mut crate::recording_encoder::FrameSchedule<RgbaImage>,
    size: (u32, u32),
) {
    if let Some((captured, frame)) = next() {
        schedule.push(captured, fit(&frame, size));
    }
}

fn start(compositor: &Compositor) -> io::Result<Session> {
    check_encoder(compositor)?;
    let frame = crate::driver_shot::frame(compositor, &chrono::Local::now().format("%H:%M").to_string())
        .map_err(|error| io::Error::other(error.to_string()))?;
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME unavailable"))?;
    let path = PathBuf::from(home)
        .join("Videos/Recordings")
        .join(chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-%f").to_string());
    let (frames, receive_frames) = mpsc::sync_channel(1);
    let (events, receive_events) = mpsc::sync_channel(256);
    let (send_result, status_receiver) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicU64::new(u64::MAX));
    let started = compositor.clock_now();
    let wall_start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mut session = Session {
        frames,
        events,
        result: status_receiver,
        stop,
        started,
        wall_start,
        next_frame: started.checked_add(INTERVAL).unwrap_or(started),
        scene: Value::Null,
        worker: None,
        timeline: None,
        injected: None,
        dropped_events: Cell::new(0),
        repeated_frames: Cell::new(0),
        sticks: RefCell::new(None),
        written_sticks: RefCell::new(String::new()),
    };
    if let Some(runtime) = &compositor.recording_runtime {
        session.injected = Some(injected(
            runtime,
            &compositor.settings().input.microphone,
            (path, wall_start),
            &frame,
            receive_frames,
            receive_events,
            send_result,
        )?);
        return Ok(session);
    }
    std::fs::create_dir_all(&path)?;
    session.timeline = Some(spawn_timeline(
        File::create_new(path.join("timeline.jsonl"))?,
        receive_events,
        Arc::clone(&session.stop),
        started,
    )?);
    session.worker = Some(start_worker(
        &session,
        path,
        frame,
        compositor.settings().input.microphone.clone(),
        receive_frames,
        send_result,
    )?);
    Ok(session)
}

fn start_worker(
    session: &Session,
    path: PathBuf,
    frame: RgbaImage,
    microphone: tehuti_core::dispatch::MicrophoneChoice,
    receive_frames: Receiver<(Duration, RgbaImage)>,
    send_result: SyncSender<Result<PathBuf, String>>,
) -> io::Result<std::thread::JoinHandle<()>> {
    let worker_stop = Arc::clone(&session.stop);
    let started = session.started;
    let wall_start = session.wall_start;
    let size = dimensions(frame.width(), frame.height());
    std::thread::Builder::new()
        .name("tehuti-recording".into())
        .spawn(move || {
            let result = (|| {
                let child = spawn_encoder(&path, size, wall_start, &microphone)?;
                encode(
                    child,
                    &path.join("video.mp4"),
                    &frame,
                    &receive_frames,
                    &worker_stop,
                    started,
                )
            })()
            .map(|()| path)
            .map_err(|error: io::Error| error.to_string());
            let _sent = send_result.send(result);
        })
}

fn check_encoder(compositor: &Compositor) -> io::Result<()> {
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    if compositor.recording_runtime.is_none()
        && !std::env::split_paths(&search_path).any(|directory| directory.join("ffmpeg").is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "ffmpeg missing: install the ffmpeg package",
        ));
    }
    Ok(())
}

fn injected(
    runtime: &std::rc::Rc<dyn crate::recording_encoder::Runtime>,
    microphone: &tehuti_core::dispatch::MicrophoneChoice,
    (path, wall_start): (PathBuf, Duration),
    frame: &RgbaImage,
    frames: Receiver<(Duration, RgbaImage)>,
    events: Receiver<Value>,
    result: SyncSender<Result<PathBuf, String>>,
) -> io::Result<Injected> {
    let size = dimensions(frame.width(), frame.height());
    std::fs::create_dir_all(&path)?;
    let sources = runtime.audio_sources(microphone)?;
    let sink = runtime.spawn(&encoder_command(&path, size, wall_start, &sources))?;
    let timeline = Timeline::new(runtime.timeline(&path.join("timeline.jsonl"))?);
    Ok(Injected {
        runtime: std::rc::Rc::clone(runtime),
        began: runtime.now(),
        sink,
        encoder: crate::recording_encoder::FrameEncoder::default(),
        schedule: crate::recording_encoder::FrameSchedule::default(),
        pixels: fit(frame, size),
        timeline,
        events,
        frames,
        size,
        result,
        path,
    })
}

/// Microphone tracks first (per the speech microphone setting), desktop sound last.
fn audio_sources(microphone: &tehuti_core::dispatch::MicrophoneChoice) -> io::Result<Vec<String>> {
    use tehuti_core::dispatch::MicrophoneChoice;
    let mut sources = match microphone {
        MicrophoneChoice::Named(name) => vec![name.clone()],
        MicrophoneChoice::Auto => vec![audio_probe("get-default-source")?],
        MicrophoneChoice::All => crate::input_choice_store::list_microphones()
            .recv_timeout(Duration::from_secs(4))
            .map_err(io::Error::other)?
            .into_iter()
            .map(|choice| choice.name)
            .collect(),
    };
    sources.push(format!("{}.monitor", audio_probe("get-default-sink")?));
    Ok(sources)
}

fn map_tracks(command: &mut Command, sources: &[String]) {
    let _video = command.args(["-map", "0:v:0"]);
    for (index, source) in sources.iter().enumerate() {
        let _track = command
            .args(["-map", &format!("{}:a:0", index.saturating_add(1))])
            .args([&format!("-metadata:s:a:{index}"), &format!("title={source}")])
            .args([
                &format!("-metadata:s:a:{index}"),
                &format!("handler_name={source}"),
            ])
            .arg(format!("-disposition:a:{index}"))
            .arg(if index == 0 { "default" } else { "0" });
    }
}

fn spawn_encoder(
    path: &std::path::Path,
    (width, height): (u32, u32),
    wall_start: Duration,
    microphone: &tehuti_core::dispatch::MicrophoneChoice,
) -> io::Result<Encoder> {
    let sources = audio_sources(microphone)?;
    std::fs::create_dir_all(path)?;
    let log = File::create_new(path.join("ffmpeg.log"))?;
    let mut command = encoder_command(path, (width, height), wall_start, &sources);
    let _stderr = command.stderr(log);
    let child = Encoder(command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::new(error.kind(), "ffmpeg missing: install the ffmpeg package")
        } else {
            error
        }
    })?);
    Ok(child)
}

/// Builds the encoder command.
///
/// Video arrives as a constant 30 fps grid starting at recording start (see
/// `FrameSchedule`). `PulseAudio` stamps audio with the wall clock (`-wallclock 1`); `-copyts`
/// keeps those stamps and `asetpts` subtracts `wall_start`, the wall-clock recording start, so
/// both streams share the timeline's zero.
fn encoder_command(
    path: &std::path::Path,
    (width, height): (u32, u32),
    wall_start: Duration,
    sources: &[String],
) -> Command {
    let mut command = Command::new("ffmpeg");
    let _args = command
        .args(["-hide_banner", "-nostdin", "-n", "-copyts"])
        .args(["-probesize", "32", "-analyzeduration", "1"])
        .args([
            "-thread_queue_size",
            "64",
            "-f",
            "rawvideo",
            "-fpsprobesize",
            "0",
            "-framerate",
            "30",
            "-pixel_format",
            "rgba",
        ])
        .args(["-video_size", &format!("{width}x{height}"), "-i", "pipe:0"]);
    for source in sources {
        let _input = command.args(["-probesize", "32", "-analyzeduration", "1"]).args([
            "-thread_queue_size",
            "64",
            "-f",
            "pulse",
            "-wallclock",
            "1",
            "-i",
            source,
        ]);
    }
    map_tracks(&mut command, sources);
    let _output = command
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "23",
            "-g",
            "30",
        ])
        .args(["-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args([
            "-af",
            &format!(
                "asetpts=PTS-{}.{:06}/TB,aresample=async=1:first_pts=0",
                wall_start.as_secs(),
                wall_start.subsec_micros()
            ),
        ])
        .args(["-shortest", "-shortest_buf_duration", "1"])
        .args(["-max_interleave_delta", "1000000"])
        .args(["-frag_duration", "1000000", "-flush_packets", "1"])
        .args(["-movflags", "+frag_keyframe+empty_moov"])
        .arg(path.join("video.mp4"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    command
}

fn audio_probe(operation: &str) -> io::Result<String> {
    let mut probe = Encoder(
        Command::new("pactl")
            .arg(operation)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                io::Error::new(error.kind(), format!("pactl (pulseaudio-utils package): {error}"))
            })?,
    );
    let deadline = Instant::now().checked_add(Duration::from_secs(2));
    loop {
        if let Some(status) = probe.0.try_wait()? {
            if !status.success() {
                return Err(io::Error::other("pactl get-default-sink failed"));
            }
            break;
        }
        if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
            return Err(io::Error::other("default audio sink lookup timed out"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut sink = String::new();
    let _bytes_read = probe
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("audio lookup stdout unavailable"))?
        .take(4096)
        .read_to_string(&mut sink)?;
    let sink = sink.trim();
    if sink.is_empty() {
        return Err(io::Error::other("default audio sink unavailable"));
    }
    Ok(sink.to_owned())
}

fn dimensions(width: u32, height: u32) -> (u32, u32) {
    let scaled_width = width.clamp(2, 1920) & !1;
    let scaled_height = u64::from(height)
        .saturating_mul(u64::from(scaled_width))
        .checked_div(u64::from(width))
        .unwrap_or(2);
    (
        scaled_width,
        u32::try_from(scaled_height).unwrap_or(u32::MAX).max(2) & !1,
    )
}

fn fit(frame: &RgbaImage, size: (u32, u32)) -> RgbaImage {
    let thumbnail = image::imageops::thumbnail(frame, size.0, size.1);
    let mut canvas = RgbaImage::from_pixel(size.0, size.1, image::Rgba([0, 0, 0, 255]));
    image::imageops::overlay(
        &mut canvas,
        &thumbnail,
        i64::from(size.0.saturating_sub(thumbnail.width()) / 2),
        i64::from(size.1.saturating_sub(thumbnail.height()) / 2),
    );
    canvas
}

fn encode(
    mut child: Encoder,
    output_path: &std::path::Path,
    frame: &RgbaImage,
    frames: &Receiver<(Duration, RgbaImage)>,
    stop: &AtomicU64,
    started: Instant,
) -> io::Result<()> {
    let result = (|| {
        let mut pipe = child
            .0
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg stdin unavailable"))?;
        rustix::fs::fcntl_setfl(&pipe, rustix::fs::OFlags::NONBLOCK)?;
        let mut encoder = crate::recording_encoder::FrameEncoder::default();
        let mut schedule = crate::recording_encoder::FrameSchedule::default();
        let size = dimensions(frame.width(), frame.height());
        let mut pixels = fit(frame, size);
        let mut drain_watch = crate::recording_encoder::DrainWatch::new(started.elapsed());
        loop {
            if child.0.try_wait()?.is_some() {
                return Err(io::Error::other(
                    "ffmpeg exited; see recording folder's ffmpeg.log",
                ));
            }
            let stopped_ms = stop.load(Ordering::Relaxed);
            take_frame(|| frames.try_recv().ok(), &mut schedule, size);
            if encoder.at_frame_boundary()
                && let Some(next) = schedule.advance(encoder.progress().0)
            {
                pixels = next;
            }
            let elapsed = video_due(started.elapsed(), stopped_ms);
            let caught_up = encoder.write_due(&mut pipe, pixels.as_raw(), || elapsed)?;
            let (written_frames, partial_bytes) = encoder.progress();
            let progress = u64::try_from(written_frames)
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::try_from(pixels.len()).unwrap_or(u64::MAX))
                .saturating_add(u64::try_from(partial_bytes).unwrap_or(u64::MAX));
            if stopped_ms != u64::MAX && drain_watch.stalled(started.elapsed(), progress) {
                return Err(io::Error::other("ffmpeg stopped accepting video frames"));
            } else if stopped_ms == u64::MAX {
                let _stalled = drain_watch.stalled(started.elapsed(), progress);
            }
            if caught_up {
                if stopped_ms != u64::MAX {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            } else {
                std::thread::yield_now();
            }
        }
        Ok(())
    })();
    finish_encoder(&mut child, output_path, result)
}

fn finish_encoder(
    child: &mut Encoder,
    output_path: &std::path::Path,
    result: io::Result<()>,
) -> io::Result<()> {
    let began = Instant::now();
    crate::recording_encoder::finish_process_with_clock(
        &mut child.0,
        output_path,
        result,
        || began.elapsed(),
        || std::thread::sleep(Duration::from_millis(20)),
    )
}

impl Session {
    fn elapsed(&self) -> Duration {
        self.injected.as_ref().map_or_else(
            || self.started.elapsed(),
            |injected| injected.runtime.now().saturating_sub(injected.began),
        )
    }

    fn pump(&mut self) {
        let elapsed = self.elapsed();
        let stopped = self.stop.load(Ordering::Relaxed);
        let Some(injected) = &mut self.injected else {
            return;
        };
        let result: io::Result<bool> = (|| {
            for event in injected.events.try_iter().take(EVENT_BATCH) {
                injected.timeline.write(&event);
            }
            injected.timeline.flush();
            take_frame(
                || injected.frames.try_recv().ok(),
                &mut injected.schedule,
                injected.size,
            );
            if injected.encoder.at_frame_boundary()
                && let Some(next) = injected.schedule.advance(injected.encoder.progress().0)
            {
                injected.pixels = next;
            }
            let elapsed = video_due(elapsed, stopped);
            if injected
                .encoder
                .write_due(&mut injected.sink, injected.pixels.as_raw(), || elapsed)?
                && stopped != u64::MAX
            {
                for event in injected.events.try_iter() {
                    injected.timeline.write(&event);
                }
                injected.timeline.finish(stopped);
                injected.sink.finish()?;
                return Ok(true);
            }
            Ok(false)
        })();
        match result {
            Ok(false) => {}
            Ok(true) => {
                let _sent = injected.result.send(Ok(injected.path.clone()));
            }
            Err(error) => {
                let _sent = injected.result.send(Err(error.to_string()));
            }
        }
    }
    fn request_stop(&self) {
        recovered(&self.dropped_events, "timeline queue drained; events dropped");
        recovered(&self.repeated_frames, "capture queue drained; frames repeated");
        let elapsed = u64::try_from(self.elapsed().as_millis()).unwrap_or(u64::MAX.saturating_sub(1));
        let _stopped = self
            .stop
            .compare_exchange(u64::MAX, elapsed, Ordering::Relaxed, Ordering::Relaxed);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take() {
            let _joined = worker.join();
        }
        if let Some(timeline) = self.timeline.take() {
            let _joined = timeline.join();
        }
    }
}

#[derive(Debug)]
struct Encoder(Child);

impl Drop for Encoder {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _waited = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    fn args(sources: &[String]) -> Vec<String> {
        let start = Duration::new(1_700_000_000, 250_000_000);
        super::encoder_command(std::path::Path::new("/x"), (64, 64), start, sources)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn after(args: &[String], flag: &str) -> Vec<String> {
        args.iter()
            .enumerate()
            .filter(|(_, arg)| *arg == flag)
            .filter_map(|(index, _)| args.get(index.saturating_add(1)).cloned())
            .collect()
    }

    #[test]
    fn video_is_a_constant_grid_from_recording_start_not_arrival_time() {
        let args = args(&[]);
        let input = args.iter().position(|arg| arg == "pipe:0").unwrap();
        assert!(!args.contains(&"-use_wallclock_as_timestamps".to_owned()));
        assert_eq!(after(&args[..input], "-framerate"), ["30"]);
        assert!(args.contains(&"-copyts".to_owned()));
    }

    #[test]
    fn audio_is_stamped_on_the_wall_clock_shifted_to_recording_start() {
        let sources = vec!["mic".to_owned(), "desk.monitor".to_owned()];
        let args = args(&sources);
        let pulse: Vec<_> = args
            .windows(5)
            .filter(|window| window[0] == "-f" && window[1] == "pulse")
            .collect();
        assert_eq!(pulse.len(), 2);
        assert!(
            pulse
                .iter()
                .all(|window| window[2] == "-wallclock" && window[3] == "1")
        );
        assert_eq!(
            after(&args, "-af"),
            ["asetpts=PTS-1700000000.250000/TB,aresample=async=1:first_pts=0"]
        );
    }

    /// Runs the production command on real ffmpeg, with each pulse input replaced by a tone the
    /// way the pulse demuxer stamps it: on the wall clock, here starting 500 ms after `wall_start`.
    /// Returns where the tone begins in the recorded file.
    fn tone_start_in_recording(folder: &std::path::Path) -> Duration {
        use std::io::Write as _;
        let wall_start = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let tone_at = wall_start.saturating_add(Duration::from_millis(500));
        let sources = vec!["tone".to_owned()];
        let production = super::encoder_command(folder, (64, 64), wall_start, &sources);
        let mut args: Vec<String> = production
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        while let Some(flag) = args.iter().position(|arg| arg == "-wallclock") {
            let _removed: Vec<_> = args.drain(flag..flag.saturating_add(2)).collect();
        }
        for arg in &mut args {
            if arg == "pulse" {
                "lavfi".clone_into(arg);
            } else if arg == "tone" {
                *arg = format!(
                    "sine=frequency=440:sample_rate=48000,asetpts=PTS+{}.{:06}/TB",
                    tone_at.as_secs(),
                    tone_at.subsec_micros()
                );
            }
        }
        let mut child = std::process::Command::new("/usr/bin/ffmpeg")
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("install ffmpeg: /usr/bin/ffmpeg missing");
        let mut pipe = child.stdin.take().unwrap();
        for _ in 0..60 {
            pipe.write_all(&[40_u8; 64 * 64 * 4]).unwrap();
        }
        drop(pipe);
        assert!(child.wait().unwrap().success());
        let probe = std::process::Command::new("/usr/bin/ffmpeg")
            .args(["-hide_banner", "-nostdin", "-i"])
            .arg(folder.join("video.mp4"))
            .args([
                "-map",
                "0:a:0",
                "-af",
                "silencedetect=n=-40dB:d=0.1",
                "-f",
                "null",
                "-",
            ])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&probe.stderr);
        let end = log
            .split("silence_end: ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .map_or(0.0, |seconds| seconds.parse::<f64>().unwrap());
        Duration::from_secs_f64(end)
    }

    #[test]
    fn audio_that_starts_late_on_the_wall_clock_lands_late_in_the_recording() {
        let folder = tempfile::tempdir().unwrap();
        let start = tone_start_in_recording(folder.path());
        assert!(
            start.abs_diff(Duration::from_millis(500)) <= Duration::from_millis(50),
            "tone captured 500 ms after recording start begins at {start:?}"
        );
    }

    #[test]
    fn a_frame_source_faster_than_the_encoder_is_polled_once_per_pass() {
        let mut polls = 0_u32;
        let mut schedule = crate::recording_encoder::FrameSchedule::default();
        super::take_frame(
            || {
                polls = polls.saturating_add(1);
                assert!(polls < 100, "the pass never ends while frames keep arriving");
                Some((Duration::ZERO, image::RgbaImage::new(4, 4)))
            },
            &mut schedule,
            (4, 4),
        );
        assert_eq!(polls, 1);
    }

    #[derive(Clone, Default)]
    struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_timeline_thread_ends_with_a_timed_stop_line_when_stopped_or_abandoned() {
        use core::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        for abandoned in [false, true] {
            let out = Shared::default();
            let (events, receive) = std::sync::mpsc::sync_channel(4);
            let stop = Arc::new(AtomicU64::new(u64::MAX));
            let thread =
                super::spawn_timeline(out.clone(), receive, Arc::clone(&stop), std::time::Instant::now())
                    .unwrap();
            events
                .send(serde_json::json!({"time_ms": 5, "kind": "key", "data": {}}))
                .unwrap();
            if abandoned {
                drop(events);
            } else {
                stop.store(1_500, Ordering::Relaxed);
            }
            thread.join().unwrap();
            let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
            let lines: Vec<serde_json::Value> = text
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(lines.len(), 2, "{text}");
            assert_eq!(lines[1]["kind"], "stop");
            let stopped = lines[1]["time_ms"].as_u64().unwrap();
            if abandoned {
                assert!(stopped < 1_000, "abandoned timeline stamped {stopped}");
            } else {
                assert_eq!(stopped, 1_500);
            }
        }
    }

    #[test]
    fn video_writing_trails_the_clock_until_stop() {
        assert_eq!(
            super::video_due(Duration::from_secs(1), u64::MAX),
            Duration::from_nanos(966_666_666)
        );
        assert_eq!(
            super::video_due(Duration::from_millis(20), u64::MAX),
            Duration::ZERO
        );
        assert_eq!(
            super::video_due(Duration::from_secs(1), 1_500),
            Duration::from_millis(1_500)
        );
    }
}
