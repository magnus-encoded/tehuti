//! Core-owned continuous adjustment sessions.

use core::ops::{Div, Mul};
use core::time::Duration;

/// The window dimension a size stepper changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeAxis {
    Width,
    Height,
}

impl SizeAxis {
    /// The spoken name that opens the stepper.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::Width => "width",
            Self::Height => "height",
        }
    }
}

impl SizeAxis {
    /// The layout command that sets the focused window to `millionths` of the working area.
    #[must_use]
    pub fn command(self, millionths: u32) -> crate::layout::Command {
        use crate::layout::{Command, SizeChange};
        let change = SizeChange::SetProportion(f64::from(millionths).div(1_000_000.0_f64));
        match self {
            Self::Width => Command::SetColumnWidth(change),
            Self::Height => Command::SetWindowHeight(change),
        }
    }

    /// The focused window's size on this axis, in millionths of the working area.
    #[must_use]
    pub fn read(self, snapshot: &crate::layout::Snapshot) -> Option<u32> {
        use crate::layout::{PresetSize, WindowHeight};
        let window = snapshot.focused_window?;
        let output = snapshot
            .outputs
            .iter()
            .find(|output| Some(output.output) == snapshot.focused_output)?;
        let workspace = output.workspaces.get(output.active_workspace)?;
        let column = workspace
            .columns
            .iter()
            .find(|column| column.tiles.iter().any(|tile| tile.window == window))?;
        let tile = column.tiles.iter().find(|tile| tile.window == window)?;
        let fraction = match self {
            Self::Width => match column.width {
                Some(PresetSize::Proportion(proportion)) => proportion,
                Some(PresetSize::Fixed(_)) | None => tile.tile.width.div(output.working_area.width),
            },
            Self::Height => match tile.height {
                WindowHeight::Proportion(proportion) => proportion,
                WindowHeight::Auto | WindowHeight::Fixed(_) => {
                    tile.tile.height.div(output.working_area.height)
                }
            },
        };
        fraction.is_finite().then(|| millionths(fraction))
    }
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=1_000_000 and rounded before the cast"
)]
fn millionths(fraction: f64) -> u32 {
    fraction.clamp(0.0_f64, 1.0_f64).mul(1_000_000.0_f64).round() as u32
}

/// Fractions of the screen a size stepper offers, in millionths: 1/3, 1/2, 2/3, 1.
pub const SIZE_STEPS: [u32; 4] = [333_333, 500_000, 666_667, 1_000_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SizeStepper {
    axis: SizeAxis,
    /// The exact size the window had, restored on revert, in millionths.
    original: u32,
    changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gauge {
    size: Option<SizeStepper>,
    pub(crate) name: String,
    minimum: i32,
    maximum: i32,
    value: i32,
    original: i32,
    pub(crate) plugin: Option<usize>,
    direction: i16,
    moved: bool,
    remainder: i64,
    idle: Duration,
}

/// How long a gauge stays up after the last change before it commits and hides.
pub const HIDE_DELAY: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum VolumeRead {
    #[default]
    Idle,
    Pending,
    Cancelled,
}

impl Gauge {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub const fn minimum(&self) -> i32 {
        self.minimum
    }
    #[must_use]
    pub const fn maximum(&self) -> i32 {
        self.maximum
    }

    // For tests and drawing only.
    #[doc(hidden)]
    #[must_use]
    pub const fn standalone(name: String, minimum: i32, maximum: i32, value: i32) -> Self {
        Self::new(name, minimum, maximum, value, None)
    }

    /// A stepper over [`SIZE_STEPS`], starting at the step nearest `current` millionths.
    pub(crate) fn size(axis: SizeAxis, current: u32) -> Self {
        let start = SIZE_STEPS
            .iter()
            .enumerate()
            .min_by_key(|(_, step)| step.abs_diff(current))
            .map_or(0, |(index, _)| index);
        let mut gauge = Self::new(
            axis.phrase().into(),
            0,
            i32::try_from(SIZE_STEPS.len().saturating_sub(1)).unwrap_or_default(),
            i32::try_from(start).unwrap_or_default(),
            None,
        );
        gauge.size = Some(SizeStepper {
            axis,
            original: current,
            changed: false,
        });
        gauge
    }

    /// Moves a size stepper by whole steps; false for other gauges or at an end.
    pub(crate) fn step(&mut self, delta: i32) -> bool {
        let Some(size) = &mut self.size else {
            return false;
        };
        let next = self.value.saturating_add(delta).clamp(self.minimum, self.maximum);
        if next == self.value {
            return false;
        }
        self.value = next;
        size.changed = true;
        true
    }

    /// The axis and size, in millionths of the screen, a size stepper currently stands for.
    #[must_use]
    pub fn fraction(&self) -> Option<(SizeAxis, u32)> {
        let size = self.size?;
        let stepped = usize::try_from(self.value)
            .ok()
            .and_then(|index| SIZE_STEPS.get(index))
            .copied()
            .filter(|_| size.changed);
        Some((size.axis, stepped.unwrap_or(size.original)))
    }

    /// True for the discrete size stepper rather than a continuous gauge.
    #[must_use]
    pub const fn is_stepper(&self) -> bool {
        self.size.is_some()
    }

    pub(crate) fn volume(value: u8) -> Self {
        Self::new("volume".into(), 0, 100, i32::from(value.min(100)), None)
    }

    pub(crate) const fn new(
        name: String,
        minimum: i32,
        maximum: i32,
        value: i32,
        plugin: Option<usize>,
    ) -> Self {
        Self {
            name,
            minimum,
            maximum,
            value,
            original: value,
            plugin,
            size: None,
            direction: 0,
            moved: false,
            remainder: 0,
            idle: Duration::ZERO,
        }
    }

    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }

    pub(crate) fn sticks(&mut self, vertical: i16) {
        if self.size.is_some() {
            return;
        }
        self.direction = vertical.clamp(-1000, 1000).saturating_neg();
        self.moved |= self.direction != 0;
        if self.direction != 0 {
            self.idle = Duration::ZERO;
        }
    }

    /// True once the level has stayed unchanged for [`HIDE_DELAY`] after being adjusted.
    pub(crate) fn expired(&self) -> bool {
        self.moved && self.idle >= HIDE_DELAY
    }

    pub(crate) fn tick(&mut self, elapsed: Duration) -> bool {
        if self.size.is_some() {
            return false;
        }
        let millis = i64::try_from(elapsed.as_millis().min(100)).unwrap_or(100);
        let range = i64::from(self.maximum).saturating_sub(i64::from(self.minimum));
        self.remainder = self.remainder.saturating_add(
            i64::from(self.direction)
                .saturating_mul(millis)
                .saturating_mul(range),
        );
        let increment = self.remainder / 2_000_000;
        self.remainder %= 2_000_000;
        let next = i64::from(self.value)
            .saturating_add(increment)
            .clamp(i64::from(self.minimum), i64::from(self.maximum));
        let next = i32::try_from(next).unwrap_or(self.value);
        let changed = self.value != next;
        self.value = next;
        if changed || self.direction != 0 {
            self.idle = Duration::ZERO;
        } else {
            self.idle = self.idle.saturating_add(elapsed);
        }
        changed
    }

    pub(crate) const fn revert(&mut self) {
        self.value = self.original;
        if let Some(size) = &mut self.size {
            size.changed = false;
        }
    }
}
