//! `[sticks]` mappings: a deflected stick fires its direction's action and repeats while held.
//!
//! Time and stick samples come in as arguments; the module holds no clock.

use core::time::Duration;
use tehuti_protocol::{Action, ButtonName, Context, Stick, StickDirection, StickMapping};

/// Deflection, in thousandths of full travel, past which a stick fires.
pub const FIRE_DEFLECTION: u32 = 500;

/// Deflection below which a fired stick counts as centred again.
pub const RELEASE_DEFLECTION: u32 = 300;

/// How long a held stick waits after its first action before repeating.
pub const REPEAT_DELAY: Duration = Duration::from_millis(400);

/// The repeat interval just past [`FIRE_DEFLECTION`].
pub const SLOWEST_REPEAT: Duration = Duration::from_millis(250);

/// The repeat interval at full deflection.
pub const FASTEST_REPEAT: Duration = Duration::from_millis(50);

/// tan(22.5°) in thousandths: the edge between a cardinal and a diagonal sector.
const SECTOR_EDGE: i32 = 414;

/// The 45° sector a stick sample points into, with the nearer cardinal for a diagonal.
#[must_use]
pub fn sector(x: i16, y: i16) -> (StickDirection, StickDirection) {
    let across = i32::from(x).saturating_abs();
    let along = i32::from(y).saturating_abs();
    let horizontal = if x >= 0 {
        StickDirection::Right
    } else {
        StickDirection::Left
    };
    let vertical = if y < 0 {
        StickDirection::Up
    } else {
        StickDirection::Down
    };
    let cardinal = if across >= along { horizontal } else { vertical };
    let exact = if along.saturating_mul(1000) <= across.saturating_mul(SECTOR_EDGE) {
        horizontal
    } else if across.saturating_mul(1000) <= along.saturating_mul(SECTOR_EDGE) {
        vertical
    } else {
        match (y < 0, x >= 0) {
            (true, true) => StickDirection::UpRight,
            (true, false) => StickDirection::UpLeft,
            (false, true) => StickDirection::DownRight,
            (false, false) => StickDirection::DownLeft,
        }
    };
    (exact, cardinal)
}

/// How far a sample is from centre, in thousandths of full travel.
#[must_use]
pub fn deflection(x: i16, y: i16) -> u32 {
    let x = i32::from(x).unsigned_abs();
    let y = i32::from(y).unsigned_abs();
    x.saturating_mul(x).saturating_add(y.saturating_mul(y)).isqrt()
}

/// The repeat interval for a deflection: faster the deeper the push.
#[must_use]
pub fn repeat_interval(deflection: u32) -> Duration {
    let depth = u64::from(deflection.clamp(FIRE_DEFLECTION, 1000).saturating_sub(FIRE_DEFLECTION));
    let span = u64::from(1000_u32.saturating_sub(FIRE_DEFLECTION)).max(1);
    let slowest = u64::try_from(SLOWEST_REPEAT.as_millis()).unwrap_or(u64::MAX);
    let fastest = u64::try_from(FASTEST_REPEAT.as_millis()).unwrap_or(0);
    let faster = slowest
        .saturating_sub(fastest)
        .saturating_mul(depth)
        .checked_div(span)
        .unwrap_or(0);
    Duration::from_millis(slowest.saturating_sub(faster))
}

/// The mapping a sample selects: the exact sector, else the nearer cardinal for an unmapped diagonal.
#[must_use]
pub fn resolve(
    context: &Context,
    stick: Stick,
    sample: (i16, i16),
    with: Option<ButtonName>,
) -> Option<&StickMapping> {
    let (exact, cardinal) = sector(sample.0, sample.1);
    context
        .stick_mapping(stick, exact, with)
        .or_else(|| context.stick_mapping(stick, cardinal, with))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Deflected {
    pointing: Option<StickDirection>,
    until_repeat: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeldModifier {
    button: ButtonName,
    stick_moved: bool,
}

/// Stick direction actions and the face buttons held as their modifiers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StickActions {
    left: Deflected,
    right: Deflected,
    modifiers: Vec<HeldModifier>,
}

impl StickActions {
    /// The modifier whose layer applies: the most recently pressed held one.
    #[must_use]
    pub fn active_modifier(&self) -> Option<ButtonName> {
        self.modifiers.last().map(|held| held.button)
    }

    #[must_use]
    pub fn holds(&self, button: ButtonName) -> bool {
        self.modifiers.iter().any(|held| held.button == button)
    }

    pub fn press_modifier(&mut self, button: ButtonName) {
        if !self.holds(button) {
            self.modifiers.push(HeldModifier {
                button,
                stick_moved: false,
            });
        }
    }

    /// Releases a held modifier; true when it was a tap without a stick move.
    pub fn release_modifier(&mut self, button: ButtonName) -> bool {
        let Some(index) = self.modifiers.iter().position(|held| held.button == button) else {
            return false;
        };
        !self.modifiers.remove(index).stick_moved
    }

    /// Forgets deflections and modifiers, as after a focus change.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advances both sticks by `elapsed` with their current samples.
    pub fn tick(
        &mut self,
        context: &Context,
        elapsed: Duration,
        left: (i16, i16),
        right: (i16, i16),
    ) -> Vec<Action> {
        let with = self.active_modifier();
        let mut actions = Vec::new();
        let mut moved = false;
        for (stick, sample, deflected) in [
            (Stick::Left, left, &mut self.left),
            (Stick::Right, right, &mut self.right),
        ] {
            if !context.maps_stick(stick) {
                *deflected = Deflected::default();
                continue;
            }
            let depth = deflection(sample.0, sample.1);
            moved |= depth >= FIRE_DEFLECTION;
            actions.extend(step_stick(context, stick, sample, depth, with, elapsed, deflected));
        }
        if moved {
            for held in &mut self.modifiers {
                held.stick_moved = true;
            }
        }
        actions
    }
}

fn step_stick(
    context: &Context,
    stick: Stick,
    sample: (i16, i16),
    depth: u32,
    with: Option<ButtonName>,
    elapsed: Duration,
    deflected: &mut Deflected,
) -> Option<Action> {
    let threshold = if deflected.pointing.is_some() {
        RELEASE_DEFLECTION
    } else {
        FIRE_DEFLECTION
    };
    if depth < threshold {
        *deflected = Deflected::default();
        return None;
    }
    let Some(mapping) = resolve(context, stick, sample, with) else {
        *deflected = Deflected::default();
        return None;
    };
    if deflected.pointing != Some(mapping.direction) {
        *deflected = Deflected {
            pointing: Some(mapping.direction),
            until_repeat: REPEAT_DELAY,
        };
        return Some(mapping.action.clone());
    }
    if !mapping.repeat {
        return None;
    }
    deflected.until_repeat = deflected.until_repeat.saturating_sub(elapsed);
    if !deflected.until_repeat.is_zero() {
        return None;
    }
    deflected.until_repeat = repeat_interval(depth);
    Some(mapping.action.clone())
}
