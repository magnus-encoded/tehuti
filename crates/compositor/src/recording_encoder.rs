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
}
