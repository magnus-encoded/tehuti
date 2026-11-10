//! Local video, separate `PulseAudio` tracks and a text timeline.
use crate::state::Compositor;
use core::{
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
        mpsc::{self, Receiver, SyncSender},
    },
    time::Instant,
};

const INTERVAL: Duration = Duration::from_nanos(33_333_334);

#[derive(Debug)]
pub(super) struct Session {
    frames: SyncSender<(Duration, RgbaImage)>,
    events: SyncSender<Value>,
    result: Receiver<Result<PathBuf, String>>,
    stop: Arc<AtomicU64>,
    started: Instant,
    next_frame: Instant,
    scene: Value,
    worker: Option<std::thread::JoinHandle<()>>,
    injected: Option<Injected>,
}

#[derive(Debug)]
struct Injected {
    runtime: std::rc::Rc<dyn crate::recording_encoder::Runtime>,
    began: Duration,
    sink: Box<dyn crate::recording_encoder::Sink>,
    encoder: crate::recording_encoder::FrameEncoder,
    pixels: RgbaImage,
    timeline: File,
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
        if self.recording_active() {
            self.record_event(kind, &json!({"event": format!("{event:?}")}));
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
        if session.events.try_send(event).is_err() {
            tracing::warn!("recording timeline queue full; dropping event");
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
        match crate::driver_shot::frame(self, &chrono::Local::now().format("%H:%M").to_string()) {
            Ok(frame) => {
                if let Some(session) = &self.recording {
                    match session.frames.try_send((session.elapsed(), frame)) {
                        Err(mpsc::TrySendError::Full(_)) => {
                            tracing::warn!("recording capture queue full; repeating the previous frame");
                        }
                        Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "recording capture failed; repeating previous frame");
            }
        }
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
    let mut session = Session {
        frames,
        events,
        result: status_receiver,
        stop,
        started,
        next_frame: started.checked_add(INTERVAL).unwrap_or(started),
        scene: Value::Null,
        worker: None,
        injected: None,
    };
    if let Some(runtime) = &compositor.recording_runtime {
        session.injected = Some(injected(
            runtime,
            &compositor.settings().input.microphone,
            path,
            &frame,
            receive_frames,
            receive_events,
            send_result,
        )?);
        return Ok(session);
    }
    session.worker = Some(start_worker(
        &session,
        path,
        frame,
        compositor.settings().input.microphone.clone(),
        (receive_frames, receive_events),
        send_result,
    )?);
    Ok(session)
}

fn start_worker(
    session: &Session,
    path: PathBuf,
    frame: RgbaImage,
    microphone: tehuti_core::dispatch::MicrophoneChoice,
    (receive_frames, receive_events): (Receiver<(Duration, RgbaImage)>, Receiver<Value>),
    send_result: SyncSender<Result<PathBuf, String>>,
) -> io::Result<std::thread::JoinHandle<()>> {
    let worker_stop = Arc::clone(&session.stop);
    let started = session.started;
    let size = dimensions(frame.width(), frame.height());
    std::thread::Builder::new()
        .name("tehuti-recording".into())
        .spawn(move || {
            let result = (|| {
                let child = spawn_encoder(&path, size, &microphone)?;
                let timeline = File::create_new(path.join("timeline.jsonl"))?;
                encode(
                    child,
                    &path.join("video.mp4"),
                    &frame,
                    timeline,
                    (&receive_frames, &receive_events),
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
    path: PathBuf,
    frame: &RgbaImage,
    frames: Receiver<(Duration, RgbaImage)>,
    events: Receiver<Value>,
    result: SyncSender<Result<PathBuf, String>>,
) -> io::Result<Injected> {
    let size = dimensions(frame.width(), frame.height());
    std::fs::create_dir_all(&path)?;
    let sources = runtime.audio_sources(microphone)?;
    let sink = runtime.spawn(&encoder_command(&path, size, &sources))?;
    let timeline = File::create_new(path.join("timeline.jsonl"))?;
    Ok(Injected {
        runtime: std::rc::Rc::clone(runtime),
        began: runtime.now(),
        sink,
        encoder: crate::recording_encoder::FrameEncoder::default(),
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
    microphone: &tehuti_core::dispatch::MicrophoneChoice,
) -> io::Result<Encoder> {
    let sources = audio_sources(microphone)?;
    std::fs::create_dir_all(path)?;
    let log = File::create_new(path.join("ffmpeg.log"))?;
    let mut command = encoder_command(path, (width, height), &sources);
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

fn encoder_command(path: &std::path::Path, (width, height): (u32, u32), sources: &[String]) -> Command {
    let mut command = Command::new("ffmpeg");
    let _args = command
        .args(["-hide_banner", "-nostdin", "-n"])
        .args(["-probesize", "32", "-analyzeduration", "1"])
        .args([
            "-thread_queue_size",
            "64",
            "-f",
            "rawvideo",
            "-fpsprobesize",
            "0",
            "-use_wallclock_as_timestamps",
            "1",
            "-pixel_format",
            "rgba",
        ])
        .args([
            "-video_size",
            &format!("{width}x{height}"),
            "-i",
            "pipe:0",
        ]);
    for source in sources {
        let _input = command.args(["-probesize", "32", "-analyzeduration", "1"]).args([
            "-thread_queue_size",
            "64",
            "-f",
            "pulse",
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
        .args(["-vf", "fps=30", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args(["-af", "aresample=async=1:first_pts=0"])
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
    mut timeline: File,
    (frames, events): (&Receiver<(Duration, RgbaImage)>, &Receiver<Value>),
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
        let size = dimensions(frame.width(), frame.height());
        let mut pixels = fit(frame, size);
        let mut drain_watch = crate::recording_encoder::DrainWatch::new(started.elapsed());
        loop {
            if child.0.try_wait()?.is_some() {
                return Err(io::Error::other(
                    "ffmpeg exited; see recording folder's ffmpeg.log",
                ));
            }
            if let Err(error) = flush_events(&mut timeline, events) {
                tracing::warn!(%error, "recording timeline write failed");
            }
            let stopped_ms = stop.load(Ordering::Relaxed);
            if encoder.at_frame_boundary()
                && let Some((_, next)) = frames.try_iter().last()
            {
                pixels = fit(&next, size);
            }
            let elapsed = if stopped_ms == u64::MAX {
                started.elapsed()
            } else {
                Duration::from_millis(stopped_ms)
            };
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
        if let Err(error) = finish_timeline(&mut timeline, events, stop.load(Ordering::Relaxed)) {
            tracing::warn!(%error, "recording timeline flush failed");
        }
        Ok(())
    })();
    finish_encoder(&mut child, output_path, result)
}

fn finish_timeline(timeline: &mut File, events: &Receiver<Value>, stopped_ms: u64) -> io::Result<()> {
    flush_events(timeline, events)?;
    serde_json::to_writer(
        &mut *timeline,
        &json!({"time_ms": stopped_ms, "kind": "stop", "data": {}}),
    )?;
    timeline.write_all(b"\n")?;
    timeline.sync_all()
}

fn flush_events(timeline: &mut File, events: &Receiver<Value>) -> io::Result<()> {
    for event in events.try_iter() {
        serde_json::to_writer(&mut *timeline, &event)?;
        timeline.write_all(b"\n")?;
    }
    Ok(())
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
            flush_events(&mut injected.timeline, &injected.events)?;
            if injected.encoder.at_frame_boundary()
                && let Some((_, next)) = injected.frames.try_iter().last()
            {
                injected.pixels = fit(&next, injected.size);
            }
            let elapsed = if stopped == u64::MAX {
                elapsed
            } else {
                Duration::from_millis(stopped)
            };
            if injected
                .encoder
                .write_due(&mut injected.sink, injected.pixels.as_raw(), || elapsed)?
                && stopped != u64::MAX
            {
                finish_timeline(&mut injected.timeline, &injected.events, stopped)?;
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
    #[test]
    fn video_frames_are_stamped_with_capture_time_not_a_fixed_rate() {
        let command = super::encoder_command(std::path::Path::new("/x"), (64, 64), &[]);
        let args: Vec<_> = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let input = args.iter().position(|arg| arg == "pipe:0").unwrap();
        let wallclock = args.iter().position(|arg| arg == "-use_wallclock_as_timestamps").unwrap();
        assert!(wallclock < input && args[wallclock + 1] == "1");
        assert!(!args[..input].contains(&"-framerate".to_owned()));
    }
}
