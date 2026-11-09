//! Frame-count pacing for a constant 30 fps recording stream.
use core::time::Duration;
use std::io::{self, Write};

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
}

/// A recording encoder whose input and completion can be driven synchronously.
pub trait Sink: Write + core::fmt::Debug {
    /// Completes the recording after all due frames have been written.
    ///
    /// # Errors
    /// Returns encoder completion failures.
    fn finish(&mut self) -> io::Result<()>;
}
