//! Quick info notices: voice jumps to a named item, controller steps through the cycle.

use core::time::Duration;

use tehuti_protocol::ButtonName;

/// How long a quick info notice stays open on its own before expiring.
pub const LIFETIME: Duration = Duration::from_secs(3);

/// One unit of quick system info shown in the on-screen notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoItem {
    Time,
    Date,
    Battery,
}

impl InfoItem {
    /// Every info item in controller cycle order.
    pub const ALL: [Self; 3] = [Self::Time, Self::Date, Self::Battery];

    /// The exact spoken phrase that jumps to this item, and the signpost shown on its notice.
    #[must_use]
    pub const fn voice_name(self) -> &'static str {
        match self {
            Self::Time => "what time is it",
            Self::Date => "date",
            Self::Battery => "battery",
        }
    }

    /// The next item in the controller cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Time => Self::Date,
            Self::Date => Self::Battery,
            Self::Battery => Self::Time,
        }
    }
}

/// The info item a whole transcript names, if any.
#[must_use]
pub fn from_phrase(phrase: &str) -> Option<InfoItem> {
    let normalized = tehuti_protocol::normalize_phrase(phrase);
    InfoItem::ALL
        .into_iter()
        .find(|item| item.voice_name() == normalized)
}

/// Charging state reported by the machine's system battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

impl BatteryStatus {
    #[must_use]
    const fn label(self) -> Option<&'static str> {
        match self {
            Self::Charging => Some("charging"),
            Self::Discharging => Some("discharging"),
            Self::Full => Some("full"),
            Self::NotCharging => Some("not charging"),
            Self::Unknown => None,
        }
    }
}

/// Charge level and state of the machine's system battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    percent: u8,
    status: BatteryStatus,
}

impl Battery {
    /// Creates a battery reading, clamping percentage to 0..=100.
    #[must_use]
    pub const fn new(percent: u8, status: BatteryStatus) -> Self {
        Self {
            percent: if percent > 100 { 100 } else { percent },
            status,
        }
    }

    #[must_use]
    pub const fn percent(self) -> u8 {
        self.percent
    }

    #[must_use]
    pub const fn status(self) -> BatteryStatus {
        self.status
    }
}

/// Local clock and battery snapshot formatted into an info notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub time: String,
    pub date: String,
    pub battery: Option<Battery>,
}

/// Formats one info notice, leading with the exact voice name for `item`.
#[must_use]
pub fn notice_text(item: InfoItem, reading: &Reading) -> String {
    let label = item.voice_name();
    match item {
        InfoItem::Time => format!("{label}: {}", reading.time),
        InfoItem::Date => format!("{label}: {}", reading.date),
        InfoItem::Battery => reading.battery.map_or_else(
            || format!("{label}: no battery"),
            |battery| {
                battery.status().label().map_or_else(
                    || format!("{label}: {}%", battery.percent()),
                    |status| format!("{label}: {}% {status}", battery.percent()),
                )
            },
        ),
    }
}

/// Pure lifecycle for an open quick info notice: direct jump, cycle, dismiss and expiry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuickInfo {
    shown: Option<InfoItem>,
    remaining: Duration,
    /// Buttons consumed while open, so their release never leaks to lower layers.
    consumed_buttons: Vec<ButtonName>,
}

impl QuickInfo {
    #[must_use]
    pub const fn shown(&self) -> Option<InfoItem> {
        self.shown
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.shown.is_some()
    }

    /// Shows `item` directly and resets the expiry timer.
    pub const fn show(&mut self, item: InfoItem) -> InfoItem {
        self.shown = Some(item);
        self.remaining = LIFETIME;
        item
    }

    /// Advances to the next item in the cycle, starting at [`InfoItem::Time`] when closed.
    pub const fn cycle(&mut self) -> InfoItem {
        let next = match self.shown {
            Some(item) => item.next(),
            None => InfoItem::Time,
        };
        self.show(next)
    }

    /// Closes an open info notice; true when one was open.
    pub const fn dismiss(&mut self) -> bool {
        let was_open = self.shown.take().is_some();
        self.remaining = Duration::ZERO;
        was_open
    }

    /// Advances the expiry timer; true when the notice just expired.
    pub const fn tick(&mut self, elapsed: Duration) -> bool {
        if self.shown.is_none() {
            return false;
        }
        self.remaining = self.remaining.saturating_sub(elapsed);
        if self.remaining.is_zero() {
            self.shown = None;
            return true;
        }
        false
    }

    pub(crate) fn claim_button(&mut self, button: ButtonName) {
        if !self.consumed_buttons.contains(&button) {
            self.consumed_buttons.push(button);
        }
    }

    pub(crate) fn release_button(&mut self, button: ButtonName) -> bool {
        let claimed = self.consumed_buttons.contains(&button);
        if claimed {
            self.consumed_buttons.retain(|held| *held != button);
        }
        claimed
    }

    pub(crate) fn clear_buttons(&mut self) {
        self.consumed_buttons.clear();
    }
}
