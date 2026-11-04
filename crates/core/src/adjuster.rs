//! Core-owned continuous adjustment sessions.

use core::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gauge {
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
    }
}
