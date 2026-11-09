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
}

impl Compositor {
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
        let event = json!({"time_ms": session.started.elapsed().as_millis(), "kind": kind, "data": data});
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
        let now = Instant::now();
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
                    match session.frames.try_send((session.started.elapsed(), frame)) {
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
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    if !std::env::split_paths(&search_path).any(|directory| directory.join("ffmpeg").is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "ffmpeg missing: install the ffmpeg package",
        ));
    }
    let frame = crate::driver_shot::frame(compositor, &chrono::Local::now().format("%H:%M").to_string())
        .map_err(|error| io::Error::other(error.to_string()))?;
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME unavailable"))?;
    let path = PathBuf::from(home)
        .join("Videos/Recordings")
        .join(chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-%f").to_string());
    let (width, height) = dimensions(frame.width(), frame.height());
    let (frames, receive_frames) = mpsc::sync_channel(1);
    let (events, receive_events) = mpsc::sync_channel(256);
    let (send_result, status_receiver) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicU64::new(u64::MAX));
    let worker_stop = Arc::clone(&stop);
    let started = Instant::now();
    let microphone = compositor.settings().input.microphone.clone();
    let worker = std::thread::Builder::new()
        .name("tehuti-recording".into())
        .spawn(move || {
            let result = (|| {
                let child = spawn_encoder(&path, (width, height), &microphone)?;
                let timeline = File::create_new(path.join("timeline.jsonl"))?;
                encode(
                    child,
                    &frame,
                    (width, height),
                    timeline,
                    (&receive_frames, &receive_events),
                    &worker_stop,
                    started,
                )
            })()
            .map(|()| path)
            .map_err(|error: io::Error| error.to_string());
            let _sent = send_result.send(result);
        })?;
    Ok(Session {
        frames,
        events,
        result: status_receiver,
        stop,
        started,
        next_frame: started.checked_add(INTERVAL).unwrap_or(started),
        scene: Value::Null,
        worker: Some(worker),
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
            "-pixel_format",
            "rgba",
        ])
        .args([
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            "30",
            "-use_wallclock_as_timestamps",
            "1",
            "-i",
            "pipe:0",
        ]);
    for source in &sources {
        let _input = command.args(["-probesize", "32", "-analyzeduration", "1"]).args([
            "-thread_queue_size",
            "64",
            "-f",
            "pulse",
            "-i",
            source,
        ]);
    }
    map_tracks(&mut command, &sources);
    let _output = command
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "23"])
        .args(["-vf", "fps=30", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args(["-af", "aresample=async=1:first_pts=0"])
        .args(["-shortest", "-movflags", "+faststart"])
        .arg(path.join("video.mp4"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(log);
    let child = Encoder(command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::new(error.kind(), "ffmpeg missing: install the ffmpeg package")
        } else {
            error
        }
    })?);
    Ok(child)
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
    frame: &RgbaImage,
    size: (u32, u32),
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
        let mut deadline = started;
        let mut pixels = fit(frame, size);
        let mut offset = 0;
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
            if stopped_ms != u64::MAX {
                break;
            }
            if offset == 0 {
                if let Some((_, next)) = frames.try_iter().last() {
                    pixels = fit(&next, size);
                }
                if Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
            }
            match pipe.write(pixels.as_raw().get(offset..).unwrap_or_default()) {
                Ok(0) => std::thread::sleep(Duration::from_millis(2)),
                Ok(count) => {
                    offset = offset.saturating_add(count);
                    if offset >= pixels.as_raw().len() {
                        offset = 0;
                        deadline = Instant::now().checked_add(INTERVAL).unwrap_or_else(Instant::now);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => {
                    tracing::warn!(%error, "recording pipe unavailable; waiting for ffmpeg");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        if let Err(error) = finish_timeline(&mut timeline, events, stop.load(Ordering::Relaxed)) {
            tracing::warn!(%error, "recording timeline flush failed");
        }
        Ok(())
    })();
    finish_encoder(&mut child, result)
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

fn finish_encoder(child: &mut Encoder, result: io::Result<()>) -> io::Result<()> {
    if result.is_err() {
        let _killed = child.0.kill();
    }
    let deadline = Instant::now().checked_add(Duration::from_secs(5));
    loop {
        if let Some(status) = child.0.try_wait()? {
            result?;
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(
                    "ffmpeg failed; see recording folder's ffmpeg.log",
                ))
            };
        }
        if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
            let _killed = child.0.kill();
            let _waited = child.0.wait();
            return Err(io::Error::other("ffmpeg did not finish within five seconds"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Session {
    fn request_stop(&self) {
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX.saturating_sub(1));
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
