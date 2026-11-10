//! Frame-count pacing for a constant 30 fps recording stream.
use core::time::Duration;
use std::collections::VecDeque;
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

/// Frames per second of the encoded video stream.
pub const FPS: u128 = 30;

/// Captured frames the schedule holds before dropping the oldest pending one.
const PENDING: usize = 16;

/// The first 30 fps slot at or after `capture`, both measured from recording start.
///
/// Slot `n` is shown at `n / 30` s, so a frame is stamped less than one frame after it was captured.
#[must_use]
pub const fn slot_of(capture: Duration) -> u128 {
    capture.as_nanos().saturating_mul(FPS).div_ceil(1_000_000_000)
}

/// Presentation time of a slot, measured from recording start.
#[must_use]
pub fn slot_time(slot: u128) -> Duration {
    let nanos = slot.saturating_mul(1_000_000_000) / FPS;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// Places captured frames on the 30 fps grid by capture time, not by arrival.
///
/// A frame that arrives late, or in a burst after a stall, still lands in the slot of its
/// capture instant; slots without a new capture repeat the previous frame.
#[derive(Debug)]
pub struct FrameSchedule<F> {
    pending: VecDeque<(u128, F)>,
    last_slot: Option<u128>,
}

impl<F> Default for FrameSchedule<F> {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            last_slot: None,
        }
    }
}

impl<F> FrameSchedule<F> {
    /// Queues a frame captured `capture` after recording start.
    ///
    /// Captures older than the newest queued one are ignored, so stamps never decrease.
    pub fn push(&mut self, capture: Duration, frame: F) {
        let slot = slot_of(capture);
        if self.last_slot.is_some_and(|last| slot < last) {
            return;
        }
        self.last_slot = Some(slot);
        if self.pending.len() >= PENDING {
            let _dropped = self.pending.pop_front();
        }
        self.pending.push_back((slot, frame));
    }

    /// The newest frame due by `slot`, if one became due since the previous call.
    pub fn advance(&mut self, slot: u128) -> Option<F> {
        let mut due = None;
        while self
            .pending
            .front()
            .is_some_and(|(due_slot, _)| *due_slot <= slot)
        {
            due = self.pending.pop_front().map(|(_, frame)| frame);
        }
        due
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
    use super::{FrameSchedule, slot_of, slot_time};
    use core::time::Duration;
    use proptest::prelude::*;

    const FRAME: Duration = Duration::from_nanos(33_333_334);

    /// Captures from gaps; then the stamp each frame gets when the encoder writes slots in
    /// bursts of `bursts` slots, after every frame captured by the burst's end has arrived.
    fn stamps(gaps: &[u64], dropped: &[usize], bursts: u128) -> (Vec<Duration>, Vec<(usize, Duration)>) {
        let mut clock = Duration::ZERO;
        let captures: Vec<_> = gaps
            .iter()
            .map(|gap| {
                clock = clock.saturating_add(Duration::from_millis(*gap));
                clock
            })
            .collect();
        let last = captures.last().map_or(0, |capture| slot_of(*capture));
        let mut schedule = FrameSchedule::default();
        let mut next: usize = 0;
        let mut shown = Vec::new();
        let mut slot: u128 = 0;
        while slot <= last {
            let end = slot.saturating_add(bursts).min(last.saturating_add(1));
            while captures.get(next).is_some_and(|capture| slot_of(*capture) < end) {
                if !dropped.contains(&next) {
                    schedule.push(captures[next], next);
                }
                next = next.saturating_add(1);
            }
            for written in slot..end {
                if let Some(frame) = schedule.advance(written) {
                    shown.push((frame, slot_time(written)));
                }
            }
            slot = end;
        }
        (captures, shown)
    }

    fn series() -> impl Strategy<Value = (Vec<u64>, u128)> {
        (
            prop::collection::vec(prop_oneof![9 => 34u64..40, 1 => 100u64..900], 1..120),
            1u128..40,
        )
    }

    proptest! {
        #[test]
        fn a_frame_is_stamped_with_its_capture_time((gaps, bursts) in series()) {
            let (captures, shown) = stamps(&gaps, &[], bursts);
            for (frame, stamp) in shown {
                let capture = captures[frame];
                prop_assert!(stamp >= capture && stamp.saturating_sub(capture) < FRAME, "{capture:?} -> {stamp:?}");
            }
        }

        #[test]
        fn stamps_never_decrease((gaps, bursts) in series()) {
            let (_, shown) = stamps(&gaps, &[], bursts);
            prop_assert!(shown.windows(2).all(|pair| pair[0].1 <= pair[1].1 && pair[0].0 < pair[1].0));
        }

        #[test]
        fn a_burst_after_a_stall_keeps_capture_spacing((gaps, bursts) in series()) {
            let (captures, shown) = stamps(&gaps, &[], bursts);
            let (_, end) = shown.last().copied().unwrap();
            let wall = *captures.last().unwrap();
            prop_assert!(end >= wall && end.saturating_sub(wall) < FRAME);
            for pair in shown.windows(2) {
                let spacing = captures[pair[1].0].saturating_sub(captures[pair[0].0]);
                let stamped = pair[1].1.saturating_sub(pair[0].1);
                prop_assert!(stamped.abs_diff(spacing) < FRAME);
            }
        }

        #[test]
        fn a_dropped_frame_leaves_a_gap((gaps, bursts) in series(), pick in any::<prop::sample::Index>()) {
            let gone = pick.index(gaps.len());
            let (_, all) = stamps(&gaps, &[], bursts);
            let (_, holed) = stamps(&gaps, &[gone], bursts);
            let kept: Vec<_> = all.into_iter().filter(|(frame, _)| *frame != gone).collect();
            prop_assert_eq!(kept, holed);
        }
    }

    #[test]
    fn late_capture_is_not_shown_before_its_time() {
        let mut schedule = FrameSchedule::default();
        schedule.push(Duration::from_millis(500), 'b');
        assert_eq!(schedule.advance(0), None);
        assert_eq!(schedule.advance(14), None);
        assert_eq!(schedule.advance(15), Some('b'));
    }
}
