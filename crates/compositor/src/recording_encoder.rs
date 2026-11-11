//! Frame-count pacing for a constant 30 fps recording stream.
use core::time::Duration;
use std::{
    io::{self, Write},
    path::Path,
    process::Child,
};

/// Tracks the last observed progress using a caller supplied clock.
#[derive(Debug)]
pub struct DrainWatch {
    last_progress: Duration,
    progress: u64,
}

impl DrainWatch {
    #[must_use]
    pub const fn new(now: Duration) -> Self {
        Self {
            last_progress: now,
            progress: 0,
        }
    }

    #[must_use]
    pub fn stalled(&mut self, now: Duration, progress: u64) -> bool {
        if progress != self.progress {
            self.progress = progress;
            self.last_progress = now;
        }
        now.saturating_sub(self.last_progress) >= Duration::from_secs(30)
    }
}

/// Waits for an encoder to finish, killing it only after 30 seconds without output growth.
/// The clock and pause callbacks allow the exact process path to be tested without waiting.
///
/// # Errors
/// Returns a child process error, the supplied recording error, or a drain timeout.
pub fn finish_process_with_clock(
    child: &mut Child,
    output_path: &Path,
    result: io::Result<()>,
    mut now: impl FnMut() -> Duration,
    mut pause: impl FnMut(),
) -> io::Result<()> {
    if result.is_err() {
        let _killed = child.kill();
    }
    let mut watch = DrainWatch::new(now());
    loop {
        if let Some(status) = child.try_wait()? {
            result?;
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(
                    "ffmpeg failed; see recording folder's ffmpeg.log",
                ))
            };
        }
        let size = std::fs::metadata(output_path).map_or(0, |metadata| metadata.len());
        if watch.stalled(now(), size) {
            let _killed = child.kill();
            let _waited = child.wait();
            return Err(io::Error::other("ffmpeg stopped finalising the recording"));
        }
        pause();
    }
}

/// Retains partial writes and catches up after encoder backpressure.
#[derive(Debug, Default)]
pub struct FrameEncoder {
    frames: u128,
    offset: usize,
}

impl FrameEncoder {
    /// Writes one available chunk, using elapsed recording time from the clock.
    ///
    /// Returns whether all frames due at that time have been written.
    ///
    /// # Errors
    /// Returns errors from the encoder, except temporary backpressure and interruptions.
    pub fn write_due(
        &mut self,
        writer: &mut impl Write,
        pixels: &[u8],
        clock: impl FnOnce() -> Duration,
    ) -> io::Result<bool> {
        let target = clock().as_nanos().saturating_mul(30) / 1_000_000_000;
        if self.offset == 0 && self.frames >= target {
            return Ok(true);
        }
        match writer.write(pixels.get(self.offset..).unwrap_or_default()) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => {
                self.offset = self.offset.saturating_add(count);
                if self.offset >= pixels.len() {
                    self.offset = 0;
                    self.frames = self.frames.saturating_add(1);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
        Ok(self.offset == 0 && self.frames >= target)
    }

    /// Whether the next write starts a new frame.
    #[must_use]
    pub const fn at_frame_boundary(&self) -> bool {
        self.offset == 0
    }

    /// Completed frames and bytes of the current frame, for drain progress.
    #[must_use]
    pub const fn progress(&self) -> (u128, usize) {
        (self.frames, self.offset)
    }
}

/// The newest capture waiting for the writer, stamped with its capture time.
///
/// Each offer replaces the waiting one, so a slow writer skips captures instead of
/// falling behind them.
#[derive(Debug)]
pub struct Latest<F>(Option<(Duration, F)>);

impl<F> Default for Latest<F> {
    fn default() -> Self {
        Self(None)
    }
}

impl<F> Latest<F> {
    /// Replaces the waiting capture, returning the one it replaced.
    pub fn offer(&mut self, captured: Duration, frame: F) -> Option<F> {
        self.0.replace((captured, frame)).map(|(_, frame)| frame)
    }

    /// Takes the waiting capture, if one arrived since the last take.
    pub const fn take(&mut self) -> Option<(Duration, F)> {
        self.0.take()
    }
}

/// A written frame is stale when its capture is older than three frame intervals.
pub const STALE: Duration = Duration::from_millis(100);

/// Written frames, and how many of them showed a stale capture.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Freshness {
    written: u64,
    stale: u64,
}

impl Freshness {
    /// Counts one written frame showing `captured` at video time `video`.
    pub fn record(&mut self, video: Duration, captured: Duration) {
        self.written = self.written.saturating_add(1);
        if video.saturating_sub(captured) > STALE {
            self.stale = self.stale.saturating_add(1);
        }
    }

    /// Whether at least a tenth of the written frames, and one second of them, were stale.
    #[must_use]
    pub const fn fell_behind(self) -> bool {
        self.stale >= 30 && self.stale.saturating_mul(10) >= self.written
    }
}

/// Writes the newest capture on the 30 fps clock and counts how fresh each frame was.
#[derive(Debug)]
pub struct FrameWriter {
    encoder: FrameEncoder,
    shown: (Duration, Vec<u8>),
    freshness: Freshness,
}

impl FrameWriter {
    /// Starts with `first`, captured at recording start.
    #[must_use]
    pub fn new(first: Vec<u8>) -> Self {
        Self {
            encoder: FrameEncoder::default(),
            shown: (Duration::ZERO, first),
            freshness: Freshness::default(),
        }
    }

    /// Writes one chunk of the frames due by video time `video`.
    ///
    /// A new frame starts from the capture `take` returns, or repeats the last one.
    /// Returns whether all due frames have been written.
    ///
    /// # Errors
    /// Returns errors from the encoder, except temporary backpressure and interruptions.
    pub fn write_due(
        &mut self,
        writer: &mut impl Write,
        take: impl FnOnce() -> Option<(Duration, Vec<u8>)>,
        video: Duration,
    ) -> io::Result<bool> {
        if self.encoder.at_frame_boundary()
            && let Some(next) = take()
        {
            self.shown = next;
        }
        let before = self.encoder.progress().0;
        let done = self.encoder.write_due(writer, &self.shown.1, || video)?;
        if self.encoder.progress().0 > before {
            self.freshness.record(video, self.shown.0);
        }
        Ok(done)
    }

    /// Bytes written so far, for drain progress.
    #[must_use]
    pub fn progress(&self) -> u64 {
        let (frames, partial) = self.encoder.progress();
        u64::try_from(frames)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::try_from(self.shown.1.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(partial).unwrap_or(u64::MAX))
    }

    /// Frames written so far.
    #[must_use]
    pub const fn frames(&self) -> u128 {
        self.encoder.progress().0
    }

    /// How fresh the written frames were.
    #[must_use]
    pub const fn freshness(&self) -> Freshness {
        self.freshness
    }
}

/// Replaceable recording clock, audio discovery and encoder process.
pub trait Runtime: core::fmt::Debug {
    /// Elapsed time on this runtime's clock.
    fn now(&self) -> Duration;
    /// Audio sources in track order.
    ///
    /// # Errors
    /// Returns audio discovery failures.
    fn audio_sources(&self, microphone: &tehuti_core::dispatch::MicrophoneChoice) -> io::Result<Vec<String>>;
    /// Opens the encoder described by the production command.
    ///
    /// # Errors
    /// Returns encoder startup failures.
    fn spawn(&self, command: &std::process::Command) -> io::Result<Box<dyn Sink>>;
    /// Opens the recording's timeline for writing.
    ///
    /// # Errors
    /// Returns file creation failures.
    fn timeline(&self, path: &Path) -> io::Result<Box<dyn Write>> {
        Ok(Box::new(std::fs::File::create_new(path)?))
    }
}

/// A recording encoder whose input and completion can be driven synchronously.
pub trait Sink: Write + core::fmt::Debug {
    /// Completes the recording after all due frames have been written.
    ///
    /// # Errors
    /// Returns encoder completion failures.
    fn finish(&mut self) -> io::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::{FrameWriter, Freshness, Latest};
    use core::time::Duration;
    use proptest::prelude::*;
    use std::io::{self, Write};

    /// Accepts the next scripted number of bytes per write; zero is backpressure.
    struct Pipe {
        accept: usize,
        bytes: Vec<u8>,
    }

    impl Write for Pipe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.accept == 0 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = bytes.len().min(self.accept);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug, Clone)]
    enum Step {
        Offer,
        Tick(u64),
        Write(usize),
        Stop,
    }

    fn ops() -> impl Strategy<Value = Vec<Step>> {
        prop::collection::vec(
            prop_oneof![
                3 => Just(Step::Offer),
                3 => (1u64..80).prop_map(Step::Tick),
                4 => (0usize..=6).prop_map(Step::Write),
                1 => Just(Step::Stop),
            ],
            1..300,
        )
    }

    /// Frame clock target at `video`, as the encoder counts it.
    fn target(video: Duration) -> u128 {
        video.as_nanos().saturating_mul(30) / 1_000_000_000
    }

    proptest! {
        /// Every written frame shows the newest capture offered before the write that began it,
        /// the frame count never runs ahead of the clock and matches it when caught up, and after
        /// stop nothing is written once the due frames are out.
        #[test]
        fn the_writer_shows_the_newest_capture_on_the_clock(ops in ops()) {
            let mut latest = Latest::default();
            let mut writer = FrameWriter::new(0u32.to_le_bytes().to_vec());
            let mut pipe = Pipe { accept: 0, bytes: Vec::new() };
            let (mut now, mut stopped, mut newest, mut finished) = (Duration::ZERO, None, 0u32, false);
            let mut expected = Vec::new();
            for step in ops {
                match step {
                    Step::Offer if stopped.is_none() => {
                        newest = newest.saturating_add(1);
                        let _replaced = latest.offer(now, newest.to_le_bytes().to_vec());
                    }
                    Step::Offer => {}
                    Step::Tick(millis) => now = now.saturating_add(Duration::from_millis(millis)),
                    Step::Stop => stopped = stopped.or(Some(now)),
                    Step::Write(accept) => {
                        pipe.accept = accept;
                        let video = stopped.map_or(now, |stop| now.min(stop));
                        let before = pipe.bytes.len();
                        let starts = before.is_multiple_of(4);
                        let done = writer.write_due(&mut pipe, || latest.take(), video).unwrap();
                        if starts && pipe.bytes.len() > before {
                            expected.push(newest);
                        }
                        prop_assert!(writer.frames() <= target(video));
                        if done {
                            prop_assert_eq!(writer.frames(), target(video));
                        }
                        if finished {
                            prop_assert_eq!(pipe.bytes.len(), before, "written after stop");
                        }
                        finished |= done && stopped.is_some();
                    }
                }
            }
            let shown: Vec<u32> = pipe
                .bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| u32::from_le_bytes(*bytes))
                .collect();
            prop_assert_eq!(&shown[..], &expected[..shown.len()]);
        }

        #[test]
        fn a_take_returns_only_the_newest_offer(offers in prop::collection::vec(prop::option::of(any::<u8>()), 0..50)) {
            let mut latest = Latest::default();
            let mut waiting = None;
            for offer in offers {
                if let Some(frame) = offer {
                    let _replaced = latest.offer(Duration::ZERO, frame);
                    waiting = Some(frame);
                } else {
                    prop_assert_eq!(latest.take().map(|(_, frame)| frame), waiting.take());
                }
            }
        }

        #[test]
        fn a_writer_that_keeps_up_never_falls_behind(frames in 1u64..2000, age in 0u64..=100) {
            let mut freshness = Freshness::default();
            for frame in 0..frames {
                let video = Duration::from_millis(frame.saturating_mul(33));
                freshness.record(video, video.saturating_sub(Duration::from_millis(age)));
            }
            prop_assert!(!freshness.fell_behind());
        }
    }

    #[test]
    fn a_writer_stale_for_a_tenth_of_a_long_take_fell_behind() {
        let mut freshness = Freshness::default();
        for frame in 0..300_u64 {
            let video = Duration::from_millis(frame.saturating_mul(33));
            let captured = if frame < 30 { Duration::ZERO } else { video };
            freshness.record(video, captured);
        }
        assert!(freshness.fell_behind());
    }
}
