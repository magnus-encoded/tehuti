//! Core-owned continuous adjustment sessions.

use core::ops::{Div, Mul};
use core::time::Duration;
use tehuti_protocol::ButtonName;

/// Minimum hardware brightness percentage, kept above zero so panels stay lit.
pub const BRIGHTNESS_MIN: u8 = 1;

/// Maximum hardware brightness percentage.
pub const BRIGHTNESS_MAX: u8 = 100;

/// Fixed percentage step for one controller nudge on brightness.
pub const BRIGHTNESS_STEP: u8 = 5;

/// How long a held D-pad direction waits before repeating brightness steps.
pub const STEP_REPEAT_DELAY: Duration = Duration::from_millis(350);

/// Interval between repeated brightness steps while a D-pad direction stays held.
pub const STEP_REPEAT_INTERVAL: Duration = Duration::from_millis(120);

/// Clamps a brightness percentage to [`BRIGHTNESS_MIN`]..=[`BRIGHTNESS_MAX`].
#[must_use]
pub fn clamp_brightness(value: u8) -> u8 {
    value.clamp(BRIGHTNESS_MIN, BRIGHTNESS_MAX)
}

/// The spoken unit name for a brightness level, matching [`parse_brightness_phrase`].
#[must_use]
pub fn brightness_unit_name(level: u8) -> String {
    let clamped = clamp_brightness(level);
    if clamped == BRIGHTNESS_MIN {
        "brightness minimum".into()
    } else if clamped == BRIGHTNESS_MAX {
        "brightness maximum".into()
    } else {
        format!("brightness {clamped}")
    }
}

/// What a spoken brightness phrase asks Tehuti to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrightnessPhrase {
    /// Open the brightness adjuster at the current hardware level.
    Open,
    /// Jump directly to an absolute clamped brightness percentage.
    Set(u8),
}

/// Parses `"brightness"`, `"brightness 40"`, `"brightness maximum"`, and `"brightness minimum"`.
///
/// Relative step words never match here (ADR 0008).
#[must_use]
pub fn parse_brightness_phrase(text: &str) -> Option<BrightnessPhrase> {
    let normalized = tehuti_protocol::normalize_phrase(text);
    if normalized == "brightness" {
        return Some(BrightnessPhrase::Open);
    }
    let rest = normalized.strip_prefix("brightness ")?;
    let target = rest.strip_suffix(" percent").unwrap_or(rest);
    if target == "maximum" {
        return Some(BrightnessPhrase::Set(BRIGHTNESS_MAX));
    }
    if target == "minimum" {
        return Some(BrightnessPhrase::Set(BRIGHTNESS_MIN));
    }
    parse_spoken_percent(target).map(|value| BrightnessPhrase::Set(clamp_brightness(value)))
}

fn parse_spoken_percent(text: &str) -> Option<u8> {
    if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
        let number = text.parse::<u32>().unwrap_or(u32::MAX);
        return Some(u8::try_from(number.min(u32::from(BRIGHTNESS_MAX))).unwrap_or(BRIGHTNESS_MAX));
    }
    match text {
        "zero" => Some(0),
        "hundred" | "one hundred" | "a hundred" => Some(BRIGHTNESS_MAX),
        _ => spoken_under_twenty(text)
            .or_else(|| spoken_tens(text))
            .or_else(|| {
                let (tens_word, ones_word) = text.split_once(' ')?;
                Some(spoken_tens(tens_word)?.saturating_add(spoken_ones(ones_word)?))
            }),
    }
}

fn spoken_ones(word: &str) -> Option<u8> {
    match word {
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        _ => None,
    }
}

fn spoken_under_twenty(word: &str) -> Option<u8> {
    if let Some(value) = spoken_ones(word) {
        return Some(value);
    }
    match word {
        "ten" => Some(10),
        "eleven" => Some(11),
        "twelve" => Some(12),
        "thirteen" => Some(13),
        "fourteen" => Some(14),
        "fifteen" => Some(15),
        "sixteen" => Some(16),
        "seventeen" => Some(17),
        "eighteen" => Some(18),
        "nineteen" => Some(19),
        _ => None,
    }
}

fn spoken_tens(word: &str) -> Option<u8> {
    match word {
        "twenty" => Some(20),
        "thirty" => Some(30),
        "forty" => Some(40),
        "fifty" => Some(50),
        "sixty" => Some(60),
        "seventy" => Some(70),
        "eighty" => Some(80),
        "ninety" => Some(90),
        _ => None,
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Volume,
    Brightness,
    Size(SizeStepper),
    Plugin(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeldStep {
    button: ButtonName,
    delta: i32,
    until_repeat: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gauge {
    kind: Kind,
    pub(crate) name: String,
    minimum: i32,
    maximum: i32,
    value: i32,
    original: i32,
    direction: i16,
    held_step: Option<HeldStep>,
    moved: bool,
    remainder: i64,
    idle: Duration,
}

/// How long a gauge stays up after the last change before it commits and hides.
pub const HIDE_DELAY: Duration = Duration::from_millis(1500);

/// Which hardware or layout read is in flight before opening a gauge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PendingRead {
    #[default]
    Idle,
    Volume,
    Brightness,
    Size(SizeAxis),
}

impl PendingRead {
    pub(crate) const fn is_pending(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

const MAX_STEP_REPEATS_PER_TICK: usize = 20;

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
        Self::with_kind(Kind::Volume, name, minimum, maximum, value)
    }

    // For tests and drawing only.
    #[doc(hidden)]
    #[must_use]
    pub fn brightness_preview(value: u8) -> Self {
        Self::brightness(value)
    }

    /// The spoken unit name shown on the bar and gauge for the current level.
    #[must_use]
    pub fn unit_label(&self) -> String {
        match self.kind {
            Kind::Brightness => {
                let level = u8::try_from(self.value).unwrap_or(BRIGHTNESS_MIN);
                brightness_unit_name(level)
            }
            Kind::Volume | Kind::Plugin(_) => format!("{} {}", self.name, self.value),
            Kind::Size(_) => self.name.clone(),
        }
    }

    /// True when the gauge accepts D-pad step nudges (brightness and size steppers).
    #[must_use]
    pub const fn supports_steps(&self) -> bool {
        matches!(self.kind, Kind::Brightness | Kind::Size(_))
    }

    /// True when this gauge controls hardware brightness.
    #[must_use]
    pub const fn is_brightness(&self) -> bool {
        matches!(self.kind, Kind::Brightness)
    }

    pub(crate) const fn plugin_id(&self) -> Option<usize> {
        match self.kind {
            Kind::Plugin(plugin) => Some(plugin),
            Kind::Volume | Kind::Brightness | Kind::Size(_) => None,
        }
    }

    /// A stepper over [`SIZE_STEPS`], starting at the step nearest `current` millionths.
    pub(crate) fn size(axis: SizeAxis, current: u32) -> Self {
        let start = SIZE_STEPS
            .iter()
            .enumerate()
            .min_by_key(|(_, step)| step.abs_diff(current))
            .map_or(0, |(index, _)| index);
        Self::with_kind(
            Kind::Size(SizeStepper {
                axis,
                original: current,
                changed: false,
            }),
            axis.phrase().into(),
            0,
            i32::try_from(SIZE_STEPS.len().saturating_sub(1)).unwrap_or_default(),
            i32::try_from(start).unwrap_or_default(),
        )
    }

    /// Applies one directional press and arms hold-repeat for brightness gauges.
    pub(crate) fn press_step(&mut self, button: ButtonName, delta: i32) -> bool {
        match &mut self.kind {
            Kind::Size(size) => {
                let next = self.value.saturating_add(delta).clamp(self.minimum, self.maximum);
                if next == self.value {
                    return false;
                }
                self.value = next;
                size.changed = true;
                true
            }
            Kind::Brightness => {
                self.held_step = Some(HeldStep {
                    button,
                    delta,
                    until_repeat: STEP_REPEAT_DELAY,
                });
                self.moved = true;
                self.idle = Duration::ZERO;
                self.step_brightness(delta)
            }
            Kind::Volume | Kind::Plugin(_) => false,
        }
    }

    /// Stops repeating when the held step button is released.
    pub(crate) fn release_button(&mut self, button: ButtonName) {
        if self.held_step.is_some_and(|hold| hold.button == button) {
            self.held_step = None;
        }
    }

    /// Jumps directly to an absolute level while the gauge is open.
    pub(crate) fn set_value(&mut self, value: i32) -> bool {
        self.held_step = None;
        self.moved = true;
        self.idle = Duration::ZERO;
        let next = value.clamp(self.minimum, self.maximum);
        let changed = self.value != next;
        self.value = next;
        changed
    }

    fn step_brightness(&mut self, delta: i32) -> bool {
        let step = delta.saturating_mul(i32::from(BRIGHTNESS_STEP));
        let next = self.value.saturating_add(step).clamp(self.minimum, self.maximum);
        let changed = self.value != next;
        self.value = next;
        changed
    }

    /// The axis and size, in millionths of the screen, a size stepper currently stands for.
    #[must_use]
    pub fn fraction(&self) -> Option<(SizeAxis, u32)> {
        let Kind::Size(size) = self.kind else {
            return None;
        };
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
        matches!(self.kind, Kind::Size(_))
    }

    pub(crate) fn volume(value: u8) -> Self {
        Self::with_kind(Kind::Volume, "volume".into(), 0, 100, i32::from(value.min(100)))
    }

    pub(crate) fn brightness(value: u8) -> Self {
        Self::with_kind(
            Kind::Brightness,
            "brightness".into(),
            i32::from(BRIGHTNESS_MIN),
            i32::from(BRIGHTNESS_MAX),
            i32::from(clamp_brightness(value)),
        )
    }

    pub(crate) const fn plugin(name: String, minimum: i32, maximum: i32, value: i32, plugin: usize) -> Self {
        Self::with_kind(Kind::Plugin(plugin), name, minimum, maximum, value)
    }

    const fn with_kind(kind: Kind, name: String, minimum: i32, maximum: i32, value: i32) -> Self {
        Self {
            kind,
            name,
            minimum,
            maximum,
            value,
            original: value,
            direction: 0,
            held_step: None,
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
        if self.is_stepper() {
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
        self.moved && self.held_step.is_none() && self.idle >= HIDE_DELAY
    }

    pub(crate) fn tick(&mut self, elapsed: Duration) -> bool {
        if self.is_stepper() {
            return false;
        }
        if let Some(mut hold) = self.held_step {
            self.idle = Duration::ZERO;
            let mut remaining = elapsed;
            let mut changed = false;
            let mut repeats = 0_usize;
            while repeats < MAX_STEP_REPEATS_PER_TICK && remaining >= hold.until_repeat {
                remaining = remaining.saturating_sub(hold.until_repeat);
                hold.until_repeat = STEP_REPEAT_INTERVAL;
                repeats = repeats.saturating_add(1);
                changed |= self.step_brightness(hold.delta);
            }
            hold.until_repeat = hold.until_repeat.saturating_sub(remaining);
            self.held_step = Some(hold);
            return changed;
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
        self.held_step = None;
        if let Kind::Size(size) = &mut self.kind {
            size.changed = false;
        }
    }
}
