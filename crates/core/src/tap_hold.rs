//! Timed synthetic key holds with caller-supplied monotonic time.
use core::time::Duration;

/// How long a tapped key stays down, so clients that poll key state see it.
pub const TAP_HOLD: core::time::Duration = core::time::Duration::from_millis(40);

/// The gap between releasing a held key and pressing it again for a re-tap: one 60 Hz frame.
pub const RETAP_GAP: core::time::Duration = core::time::Duration::from_millis(17);

/// Whether the caller has an outstanding release timer.
#[derive(Debug, PartialEq, Eq)]
enum ReleaseTimer {
    Idle,
    Armed,
}

/// A key pressed by a tap, released once its hold time passes.
#[derive(Debug)]
struct HeldKey<Key> {
    code: Key,
    due: Duration,
    modifier: bool,
    /// False while a re-tap waits one frame after the release before pressing again.
    pressed: bool,
    /// The focus epoch when the tap began; a later focus change releases it.
    epoch: u32,
}

#[derive(Debug)]
/// Synthetic holds independent of the key representation and clock source.
pub struct TapHold<Key> {
    held_taps: Vec<HeldKey<Key>>,
    release_timer: ReleaseTimer,
    focus_epoch: u32,
    hold_duration: Duration,
}

impl<Key: Copy + Eq> Default for TapHold<Key> {
    fn default() -> Self {
        Self {
            held_taps: Vec::new(),
            release_timer: ReleaseTimer::Idle,
            focus_epoch: 0,
            hold_duration: TAP_HOLD,
        }
    }
}

impl<Key: Copy + Eq> TapHold<Key> {
    pub const fn instant_taps(&mut self) {
        self.hold_duration = Duration::ZERO;
    }
    /// Extends shared modifiers and restarts repeated keys after a release gap.
    pub fn tap(&mut self, now: Duration, keycode: Key, modifiers: &[Key]) -> Vec<(Key, bool)> {
        let duration = self.hold_duration;
        let due = if duration.is_zero() {
            None
        } else {
            now.checked_add(duration)
        };
        let Some(due) = due else {
            return modifiers
                .iter()
                .copied()
                .map(|code| (code, true))
                .chain([(keycode, true), (keycode, false)])
                .chain(modifiers.iter().rev().copied().map(|code| (code, false)))
                .collect();
        };
        let mut events = Vec::new();
        let epoch = self.focus_epoch;
        for code in modifiers {
            if let Some(held) = self.held_taps.iter_mut().find(|held| held.code == *code) {
                held.due = due;
            } else {
                events.push((*code, true));
                self.held_taps.push(HeldKey {
                    code: *code,
                    due,
                    modifier: true,
                    pressed: true,
                    epoch,
                });
            }
        }
        if let Some(index) = self.held_taps.iter().position(|held| held.code == keycode) {
            let held = self.held_taps.remove(index);
            if held.pressed {
                events.push((keycode, false));
            }
            let gap = now.checked_add(RETAP_GAP).unwrap_or(due);
            self.held_taps.push(HeldKey {
                code: keycode,
                due: gap,
                modifier: false,
                pressed: false,
                epoch,
            });
            return events;
        }
        events.push((keycode, true));
        self.held_taps.push(HeldKey {
            code: keycode,
            due,
            modifier: false,
            pressed: true,
            epoch,
        });
        events
    }

    /// Advances deferred presses and releases due holds, keys before modifiers.
    pub fn tick(&mut self, now: Duration) -> Vec<(Key, bool)> {
        let mut events = Vec::new();
        for held in &mut self.held_taps {
            if !held.pressed && held.due <= now {
                events.push((held.code, true));
                held.pressed = true;
                held.epoch = self.focus_epoch;
                held.due = now.checked_add(self.hold_duration).unwrap_or(now);
            }
        }
        events.extend(self.release(|held| held.due <= now));
        events
    }

    pub fn release_all(&mut self) -> Vec<(Key, bool)> {
        self.release(|_| true)
    }

    /// Records focus while the caller cannot yet deliver key releases.
    pub const fn focus_changed(&mut self) {
        self.focus_epoch = self.focus_epoch.wrapping_add(1);
    }

    /// Releases holds from earlier focus generations.
    pub fn flush_focus(&mut self) -> Vec<(Key, bool)> {
        let epoch = self.focus_epoch;
        self.release(|held| held.epoch < epoch)
    }

    fn release(&mut self, selected: impl Fn(&HeldKey<Key>) -> bool) -> Vec<(Key, bool)> {
        let (mut released, waiting): (Vec<_>, Vec<_>) = core::mem::take(&mut self.held_taps)
            .into_iter()
            .partition(selected);
        self.held_taps = waiting;
        released.sort_by_key(|held| held.modifier);
        released
            .into_iter()
            .filter(|held| held.pressed)
            .map(|held| (held.code, false))
            .collect()
    }

    pub fn timer_failed(&mut self) -> Vec<(Key, bool)> {
        self.release_timer = ReleaseTimer::Idle;
        self.release_all()
    }

    pub fn take_release_request(&mut self, now: Duration) -> Option<Duration> {
        if self.release_timer == ReleaseTimer::Armed {
            return None;
        }
        let due = self.held_taps.iter().map(|held| held.due).min()?;
        self.release_timer = ReleaseTimer::Armed;
        Some(due.saturating_sub(now))
    }

    pub fn timer_fired(&mut self, now: Duration) -> (Vec<(Key, bool)>, Option<Duration>) {
        let events = self.tick(now);
        let due = self.held_taps.iter().map(|held| held.due).min();
        self.release_timer = if due.is_some() {
            ReleaseTimer::Armed
        } else {
            ReleaseTimer::Idle
        };
        (
            events,
            due.map(|due| due.saturating_sub(now).max(Duration::from_millis(1))),
        )
    }
}
