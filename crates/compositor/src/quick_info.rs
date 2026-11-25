//! Local clock and `/sys/class/power_supply` reader for quick info notices.

use core::fmt;
use std::{
    io::Read as _,
    path::{Path, PathBuf},
};

use tehuti_core::quick_info::{Battery, BatteryStatus, InfoItem, Reading, notice_text};

const POWER_SUPPLY_PATH: &str = "/sys/class/power_supply";
const MAX_SYSFS_BYTES: u64 = 256;

/// Reads local time, date and system battery state for quick info notices.
pub struct Source {
    power_supply: PathBuf,
    clock: Box<dyn Fn() -> (String, String)>,
}

impl fmt::Debug for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Source")
            .field("power_supply", &self.power_supply)
            .finish_non_exhaustive()
    }
}

impl Default for Source {
    fn default() -> Self {
        Self::system()
    }
}

impl Source {
    /// Reads the machine's local clock and `/sys/class/power_supply`.
    #[must_use]
    pub fn system() -> Self {
        Self::with_paths_and_clock(PathBuf::from(POWER_SUPPLY_PATH), || {
            let now = chrono::Local::now();
            (
                now.format("%H:%M").to_string(),
                now.format("%a %Y-%m-%d").to_string(),
            )
        })
    }

    /// Uses a supplied power-supply directory and clock function, for tests without hardware.
    #[must_use]
    pub fn with_paths_and_clock(
        power_supply: PathBuf,
        clock: impl Fn() -> (String, String) + 'static,
    ) -> Self {
        Self {
            power_supply,
            clock: Box::new(clock),
        }
    }

    /// Samples the current clock and battery state.
    #[must_use]
    pub fn read(&self) -> Reading {
        let (time, date) = (self.clock)();
        Reading {
            time,
            date,
            battery: read_battery(&self.power_supply),
        }
    }

    /// Formats the on-screen notice text for `item`.
    #[must_use]
    pub fn notice(&self, item: InfoItem) -> String {
        notice_text(item, &self.read())
    }
}

/// Reads system battery charge and state from a Linux `power_supply` directory.
///
/// Peripheral batteries (`scope = Device`) and mains adapters are skipped. Returns `None`
/// when no system battery is present.
#[must_use]
pub fn read_battery(power_supply: &Path) -> Option<Battery> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(power_supply)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    let batteries: Vec<Battery> = entries
        .into_iter()
        .filter_map(|entry| read_one_battery(&entry))
        .collect();
    combine_batteries(&batteries)
}

fn read_one_battery(directory: &Path) -> Option<Battery> {
    if read_small(&directory.join("type")).as_deref() != Some("Battery") {
        return None;
    }
    if read_small(&directory.join("scope")).is_some_and(|scope| scope.eq_ignore_ascii_case("device")) {
        return None;
    }
    let percent = read_small(&directory.join("capacity"))
        .and_then(|value| value.parse::<u8>().ok())
        .or_else(|| percent_from_pair(directory, "energy_now", "energy_full"))
        .or_else(|| percent_from_pair(directory, "charge_now", "charge_full"))?;
    let status = parse_status(read_small(&directory.join("status")).as_deref());
    Some(Battery::new(percent, status))
}

fn combine_batteries(batteries: &[Battery]) -> Option<Battery> {
    let count = u64::try_from(batteries.len()).ok()?;
    if count == 0 {
        return None;
    }
    let total = batteries.iter().fold(0u64, |sum, battery| {
        sum.saturating_add(u64::from(battery.percent()))
    });
    let average = total.checked_div(count)?;
    let percent = u8::try_from(average).ok()?;
    let status = if batteries
        .iter()
        .any(|battery| battery.status() == BatteryStatus::Charging)
    {
        BatteryStatus::Charging
    } else if batteries
        .iter()
        .any(|battery| battery.status() == BatteryStatus::Discharging)
    {
        BatteryStatus::Discharging
    } else if batteries
        .iter()
        .all(|battery| battery.status() == BatteryStatus::Full)
    {
        BatteryStatus::Full
    } else if batteries
        .iter()
        .any(|battery| battery.status() == BatteryStatus::NotCharging)
    {
        BatteryStatus::NotCharging
    } else {
        BatteryStatus::Unknown
    };
    Some(Battery::new(percent, status))
}

fn percent_from_pair(directory: &Path, now_name: &str, full_name: &str) -> Option<u8> {
    let now: u128 = read_small(&directory.join(now_name))?.parse().ok()?;
    let full: u128 = read_small(&directory.join(full_name))?.parse().ok()?;
    if full == 0 {
        return None;
    }
    let percent = now.saturating_mul(100).checked_div(full)?.min(100);
    u8::try_from(percent).ok()
}

fn parse_status(raw: Option<&str>) -> BatteryStatus {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("charging") => BatteryStatus::Charging,
        Some("discharging") => BatteryStatus::Discharging,
        Some("full") => BatteryStatus::Full,
        Some("not charging") => BatteryStatus::NotCharging,
        Some(_) | None => BatteryStatus::Unknown,
    }
}

fn read_small(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    let _bytes = file.take(MAX_SYSFS_BYTES).read_to_string(&mut text).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
