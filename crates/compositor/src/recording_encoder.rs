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
