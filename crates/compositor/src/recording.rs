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
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    time::Instant,
};

type Frames = Arc<Mutex<crate::recording_encoder::Latest<RgbaImage>>>;
type Saved = Result<(PathBuf, crate::recording_encoder::Freshness), String>;

/// The capture slot; a panic while it was held leaves a whole frame or none, so it is reused.
fn slot(frames: &Frames) -> std::sync::MutexGuard<'_, crate::recording_encoder::Latest<RgbaImage>> {
    frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

const INTERVAL: Duration = Duration::from_nanos(33_333_334);

/// Gives capture one interval to arrive before writing the next video slot.
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
    frames: Frames,
    events: SyncSender<Value>,
    result: Receiver<Saved>,
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
    /// Continuous samples (sticks, pointer motion, scroll), written at most once per video frame.
    samples: RefCell<Vec<Sample>>,
}

#[derive(Debug)]
struct Injected {
    runtime: std::rc::Rc<dyn crate::recording_encoder::Runtime>,
    began: Duration,
    sink: Box<dyn crate::recording_encoder::Sink>,
    writer: crate::recording_encoder::FrameWriter,
    timeline: Timeline<Box<dyn Write>>,
    events: Receiver<Value>,
    frames: Frames,
    size: (u32, u32),
    result: SyncSender<Saved>,
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

    /// Keeps the newest continuous sample of `name`; `poll_recording` writes it once per video
    /// frame, and only when it changed. Discrete events go through `record_debug` unfiltered.
    pub(super) fn record_sample(
        &self,
        kind: &'static str,
        name: &'static str,
        sample: &impl core::fmt::Debug,
    ) {
        let Some(session) = &self.recording else { return };
        if session.stop.load(Ordering::Relaxed) != u64::MAX {
            return;
        }
        let text = format!("{sample:?}");
        let mut samples = session.samples.borrow_mut();
        if let Some(slot) = samples.iter_mut().find(|slot| slot.name == name) {
            slot.pending = Some(text);
        } else {
            samples.push(Sample {
                kind,
                name,
                pending: Some(text),
                written: String::new(),
            });
        }
    }

    fn flush_samples(&self) {
        let due: Vec<(&'static str, String)> = self.recording.as_ref().map_or_else(Vec::new, |session| {
            session
                .samples
                .borrow_mut()
                .iter_mut()
                .filter_map(|slot| {
                    let text = slot.pending.take().filter(|text| *text != slot.written)?;
                    slot.written.clone_from(&text);
                    Some((slot.kind, text))
                })
                .collect()
        });
        for (kind, text) in due {
            self.record_event(kind, &json!({"event": text}));
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
                Ok((path, freshness)) => self.show_notice(&saved_notice(&path, freshness)),
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
        self.flush_samples();
        let captured = self.recording.as_ref().map_or(Duration::ZERO, Session::elapsed);
        match crate::driver_shot::frame(self, &chrono::Local::now().format("%H:%M").to_string()) {
            Ok(frame) => {
                if let Some(session) = &self.recording {
                    let replaced = slot(&session.frames).offer(captured, frame);
                    drop(replaced);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "recording capture failed; repeating previous frame");
            }
        }
    }
}

fn saved_notice(path: &std::path::Path, freshness: crate::recording_encoder::Freshness) -> String {
    let log = std::fs::read_to_string(path.join("ffmpeg.log")).unwrap_or_default();
    let mut notice = format!("RECORDING SAVED: {}", path.display());
    for warning in silent_tracks(&log) {
        notice.push_str("\nWARNING: ");
        notice.push_str(warning);
    }
    if freshness.fell_behind() {
        notice.push_str("\nWARNING: VIDEO FELL BEHIND; PARTS OF THE PICTURE ARE STALE");
    }
    notice
}

/// Tracks whose `astats` peak was digital silence; the desktop track is mapped last.
fn silent_tracks(log: &str) -> Vec<&'static str> {
    let peaks: Vec<bool> = log
        .lines()
        .filter_map(|line| line.split_once("Peak level dB: "))
        .map(|(_, level)| level.trim() == "-inf")
        .collect();
    let Some((desktop, microphones)) = peaks.split_last() else {
        return Vec::new();
    };
    let mut silent = Vec::new();
    if microphones.iter().any(|silent| *silent) {
        silent.push("MIC TRACK SILENT");
    }
    if *desktop {
        silent.push("DESKTOP TRACK SILENT");
    }
    silent
}

/// The newest value of one continuous input and the last one written.
#[derive(Debug)]
struct Sample {
    kind: &'static str,
    name: &'static str,
    pending: Option<String>,
    written: String,
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

/// Takes the newest waiting capture, scaled to the video size, when the writer starts a frame.
///
/// Only the frame the writer uses is scaled; captures it skipped cost nothing here.
fn take_frame(frames: &Frames, size: (u32, u32)) -> Option<(Duration, Vec<u8>)> {
    let (captured, frame) = slot(frames).take()?;
    let frame = if frame.dimensions() == size {
        frame
    } else {
        fit(&frame, size)
    };
    Some((captured, frame.into_raw()))
}

fn start(compositor: &Compositor) -> io::Result<Session> {
    check_encoder(compositor)?;
    let frame = crate::driver_shot::frame(compositor, &chrono::Local::now().format("%H:%M").to_string())
        .map_err(|error| io::Error::other(error.to_string()))?;
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME unavailable"))?;
    let path = PathBuf::from(home)
        .join("Videos/Recordings")
        .join(chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-%f").to_string());
    let frames = Arc::new(Mutex::new(crate::recording_encoder::Latest::default()));
    let receive_frames = Arc::clone(&frames);
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
        samples: RefCell::new(Vec::new()),
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
    receive_frames: Frames,
    send_result: SyncSender<Saved>,
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
            .map(|freshness| (path, freshness))
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
    frames: Frames,
    events: Receiver<Value>,
    result: SyncSender<Saved>,
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
        writer: crate::recording_encoder::FrameWriter::new(fit(frame, size).into_raw()),
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
/// Video uses the newest complete capture on a constant 30 fps grid from recording start.
/// Backpressure replaces waiting captures without adding video slots.
/// `PulseAudio` stamps audio with the wall clock (`-wallclock 1`); `-copyts`
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
                "asetpts=PTS-{}.{:06}/TB,aresample=async=1:first_pts=0,astats=measure_perchannel=none:measure_overall=Peak_level",
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
    if frame.dimensions() == size {
        return frame.clone();
    }
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
    frames: &Frames,
    stop: &AtomicU64,
    started: Instant,
) -> io::Result<crate::recording_encoder::Freshness> {
    let result = (|| {
        let mut pipe = child
            .0
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg stdin unavailable"))?;
        rustix::fs::fcntl_setfl(&pipe, rustix::fs::OFlags::NONBLOCK)?;
        let size = dimensions(frame.width(), frame.height());
        let mut writer = crate::recording_encoder::FrameWriter::new(fit(frame, size).into_raw());
        let mut drain_watch = crate::recording_encoder::DrainWatch::new(started.elapsed());
        loop {
            if child.0.try_wait()?.is_some() {
                return Err(io::Error::other(
                    "ffmpeg exited; see recording folder's ffmpeg.log",
                ));
            }
            let stopped_ms = stop.load(Ordering::Relaxed);
            let elapsed = video_due(started.elapsed(), stopped_ms);
            let caught_up = writer.write_due(&mut pipe, || take_frame(frames, size), elapsed)?;
            let progress = writer.progress();
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
        Ok(writer.freshness())
    })();
    let freshness = result.as_ref().map_or_else(|_| Default::default(), |freshness| *freshness);
    finish_encoder(&mut child, output_path, result.map(|_| ())).map(|()| freshness)
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
            let elapsed = video_due(elapsed, stopped);
            let (frames, size) = (&injected.frames, injected.size);
            if injected
                .writer
                .write_due(&mut injected.sink, || take_frame(frames, size), elapsed)?
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
                let _sent = injected
                    .result
                    .send(Ok((injected.path.clone(), injected.writer.freshness())));
            }
            Err(error) => {
                let _sent = injected.result.send(Err(error.to_string()));
            }
        }
    }
    fn request_stop(&self) {
        recovered(&self.dropped_events, "timeline queue drained; events dropped");
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

    #[test]
    fn silent_tracks_are_named_by_position_with_the_desktop_last() {
        let peak = |level: &str| format!("[Parsed_astats_2 @ 0x1] Peak level dB: {level}\n");
        for (levels, silent) in [
            (vec![], vec![]),
            (vec!["-30.8", "-4.5"], vec![]),
            (vec!["-inf", "-4.5"], vec!["MIC TRACK SILENT"]),
            (vec!["-30.8", "-inf"], vec!["DESKTOP TRACK SILENT"]),
            (vec!["-30.8", "-inf", "-inf"], vec!["MIC TRACK SILENT", "DESKTOP TRACK SILENT"]),
        ] {
            let log: String = levels.iter().map(|level| peak(level)).collect();
            assert_eq!(super::silent_tracks(&log), silent, "{log}");
        }
    }

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
            [
                "asetpts=PTS-1700000000.250000/TB,aresample=async=1:first_pts=0,astats=measure_perchannel=none:measure_overall=Peak_level"
            ]
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
