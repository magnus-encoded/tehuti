//! Local video, separate PulseAudio tracks and a text timeline.
use crate::state::Compositor;
use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use image::RgbaImage;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, Write},
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
    frames: SyncSender<RgbaImage>,
    events: SyncSender<Value>,
    result: Receiver<Result<PathBuf, String>>,
    stop: Arc<AtomicBool>,
    started: Instant,
    next_frame: Instant,
    scene: Value,
}

impl Compositor {
    pub(super) fn recording_active(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|session| !session.stop.load(Ordering::Relaxed))
    }

    pub(super) fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        match start(self) {
            Ok(session) => {
                self.recording = Some(session);
                self.record_event("start", json!({}));
                self.poll_recording();
            }
            Err(error) => self.show_notice(&format!("RECORDING FAILED: {error}")),
        }
    }

    pub(super) fn stop_recording(&mut self) {
        self.record_event("stop", json!({}));
        if let Some(session) = &self.recording {
            session.stop.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn record_event(&mut self, kind: &str, data: Value) {
        let Some(session) = &self.recording else { return };
        if session.stop.load(Ordering::Relaxed) {
            return;
        }
        let event = json!({"time_ms": session.started.elapsed().as_millis(), "kind": kind, "data": data});
        if session.events.try_send(event).is_err() {
            session.stop.store(true, Ordering::Relaxed);
            self.show_notice("RECORDING STOPPED: timeline writer unavailable or too slow");
        }
    }

    pub(super) fn record_screen(&mut self) {
        if !self.recording_active() {
            return;
        }
        let mut scene = self.driver_scene();
        scene["clock"] = json!(chrono::Local::now().format("%H:%M").to_string());
        if let Some(object) = scene.as_object_mut() {
            let _clock = object.remove("clock_ms");
        }
        if self
            .recording
            .as_ref()
            .is_some_and(|session| session.scene != scene)
        {
            self.record_event("screen", scene.clone());
            if let Some(session) = &mut self.recording {
                session.scene = scene;
            }
        }
    }

    pub(super) fn poll_recording(&mut self) {
        let result = self
            .recording
            .as_ref()
            .and_then(|session| session.result.try_recv().ok());
        if let Some(result) = result {
            self.recording = None;
            match result {
                Ok(path) => self.show_notice(&format!("RECORDING SAVED: {}", path.display())),
                Err(error) => self.show_notice(&format!("RECORDING FAILED: {error}")),
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
                    match session.frames.try_send(frame) {
                        Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                        Err(mpsc::TrySendError::Disconnected(_)) => self.stop_recording(),
                    }
                }
            }
            Err(error) => {
                self.stop_recording();
                self.show_notice(&format!("RECORDING CAPTURE FAILED: {error}"));
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
    let sink = default_monitor()?;
    std::fs::create_dir_all(&path)?;
    let timeline = File::create_new(path.join("timeline.jsonl"))?;
    let log = File::create_new(path.join("ffmpeg.log"))?;
    let mut command = Command::new("ffmpeg");
    let _args = command
        .args([
            "-hide_banner",
            "-nostdin",
            "-n",
            "-thread_queue_size",
            "64",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            "30",
            "-i",
            "pipe:0",
            "-thread_queue_size",
            "64",
            "-f",
            "pulse",
            "-i",
            &sink,
            "-thread_queue_size",
            "64",
            "-f",
            "pulse",
            "-i",
            "default",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-map",
            "2:a:0",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-metadata:s:a:0",
            "title=System output",
            "-metadata:s:a:1",
            "title=Microphone",
            "-shortest",
            "-movflags",
            "+faststart",
        ])
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
    let (frames, receive_frames) = mpsc::sync_channel(1);
    let (events, receive_events) = mpsc::sync_channel(256);
    let (send_result, result) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let started = Instant::now();
    let _worker = std::thread::Builder::new()
        .name("tehuti-recording".into())
        .spawn(move || {
            let result = encode(
                child,
                frame,
                (width, height),
                timeline,
                &receive_frames,
                &receive_events,
                &worker_stop,
            )
            .map(|()| path)
            .map_err(|error| error.to_string());
            let _sent = send_result.send(result);
        })?;
    Ok(Session {
        frames,
        events,
        result,
        stop,
        started,
        next_frame: started,
        scene: Value::Null,
    })
}

fn default_monitor() -> io::Result<String> {
    let result = Command::new("pactl").arg("get-default-sink").output()?;
    if !result.status.success() {
        return Err(io::Error::other(
            "cannot find system audio: pactl get-default-sink failed",
        ));
    }
    let sink = String::from_utf8(result.stdout).map_err(io::Error::other)?;
    let sink = sink.trim();
    if sink.is_empty() {
        return Err(io::Error::other("default audio sink unavailable"));
    }
    Ok(format!("{sink}.monitor"))
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
    mut frame: RgbaImage,
    size: (u32, u32),
    mut timeline: File,
    frames: &Receiver<RgbaImage>,
    events: &Receiver<Value>,
    stop: &AtomicBool,
) -> io::Result<()> {
    let result = (|| {
        let mut pipe = child
            .0
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg stdin unavailable"))?;
        rustix::fs::fcntl_setfl(&pipe, rustix::fs::OFlags::NONBLOCK)?;
        let mut deadline = Instant::now();
        let mut pixels = fit(&frame, size);
        loop {
            for event in events.try_iter() {
                serde_json::to_writer(&mut timeline, &event)?;
                timeline.write_all(b"\n")?;
            }
            match frames.try_recv() {
                Ok(next) => {
                    frame = next;
                    pixels = fit(&frame, size);
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            write_frame(&mut pipe, pixels.as_raw())?;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            deadline = deadline.checked_add(INTERVAL).unwrap_or_else(Instant::now);
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        }
        timeline.sync_all()?;
        Ok(())
    })();
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

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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

fn write_frame(pipe: &mut impl Write, mut pixels: &[u8]) -> io::Result<()> {
    let deadline = Instant::now().checked_add(Duration::from_secs(2));
    while !pixels.is_empty() {
        match pipe.write(pixels) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => pixels = pixels.get(count..).unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
                    return Err(io::Error::other("ffmpeg stopped accepting frames"));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
