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
}

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
        }
    }

    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }

    pub(crate) fn sticks(&mut self, vertical: i16) -> bool {
        self.direction = vertical.clamp(-1000, 1000).saturating_neg();
        self.moved |= self.direction != 0;
        self.moved && self.direction == 0
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
        changed
    }

    pub(crate) const fn revert(&mut self) {
        self.value = self.original;
    }
}
