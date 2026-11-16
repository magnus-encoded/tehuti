//! How much a recording timeline writes: `info` by default, `debug` for every per-frame event.

use std::path::{Path, PathBuf};

/// Stick travel, in thousandths, a stick must move from the last kept reading to be written at `info`.
pub const STICK_THRESHOLD: i16 = 50;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimelineLevel {
    /// No per-frame events: sticks only past a threshold, no repeated invitation closes.
    #[default]
    Info,
    /// Everything, so a single bad frame can be pinned down.
    Debug,
}

impl TimelineLevel {
    /// Reads `timeline = "info" | "debug"` from the text of `recording.toml`.
    ///
    /// # Errors
    /// Returns a message when the text is not TOML or names an unknown level.
    pub fn parse(text: &str) -> Result<Self, String> {
        let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
        match table.get("timeline").map(toml::Value::as_str) {
            None | Some(Some("info")) => Ok(Self::Info),
            Some(Some("debug")) => Ok(Self::Debug),
            Some(_) => Err("timeline must be \"info\" or \"debug\"".into()),
        }
    }

    /// Loads `recording.toml` from the config directory; a missing or bad file means `info`.
    #[must_use]
    pub fn load() -> Self {
        config_path().map_or(Self::Info, |path| read(&path))
    }

    /// Whether a stick reading is worth a line at this level, given the last reading kept.
    #[must_use]
    pub fn keeps_sticks(self, last: Option<[i16; 4]>, now: [i16; 4]) -> bool {
        match (self, last) {
            (Self::Debug, _) | (Self::Info, None) => true,
            (Self::Info, Some(last)) => {
                let moved = last
                    .iter()
                    .zip(now)
                    .any(|(before, after)| before.abs_diff(after) >= STICK_THRESHOLD.unsigned_abs());
                // Returning to rest is always kept, so a reader sees the stick let go.
                moved || (now == [0; 4] && last != now)
            }
        }
    }
}

fn read(path: &Path) -> TimelineLevel {
    match std::fs::read_to_string(path) {
        Ok(text) => TimelineLevel::parse(&text).unwrap_or_else(|error| {
            tracing::warn!(%error, path = %path.display(), "recording config ignored; timeline stays at info");
            TimelineLevel::Info
        }),
        Err(_) => TimelineLevel::Info,
    }
}

fn config_path() -> Option<PathBuf> {
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(directory.join("tehuti/recording.toml"))
}

#[cfg(test)]
mod tests {
    use super::{STICK_THRESHOLD, TimelineLevel};
    use proptest::prelude::*;

    fn reading() -> impl Strategy<Value = [i16; 4]> {
        prop::array::uniform4(-1000_i16..=1000)
    }

    proptest! {
        #[test]
        fn debug_keeps_every_reading(last in reading(), now in reading()) {
            prop_assert!(TimelineLevel::Debug.keeps_sticks(Some(last), now));
        }

        #[test]
        fn info_drops_jitter_below_the_threshold(last in reading(), jitter in prop::array::uniform4(0_i16..STICK_THRESHOLD)) {
            let now = [0, 1, 2, 3].map(|axis| (last[axis] + jitter[axis]).min(1000));
            prop_assume!(now != [0; 4]);
            prop_assert!(!TimelineLevel::Info.keeps_sticks(Some(last), now));
        }

        #[test]
        fn info_keeps_a_move_past_the_threshold(last in reading(), axis in 0_usize..4) {
            let mut now = last;
            now[axis] = if last[axis] > 0 { last[axis] - STICK_THRESHOLD } else { last[axis] + STICK_THRESHOLD };
            prop_assert!(TimelineLevel::Info.keeps_sticks(Some(last), now));
        }
    }

    #[test]
    fn info_keeps_the_first_reading_and_the_return_to_rest() {
        assert!(TimelineLevel::Info.keeps_sticks(None, [1, 0, 0, 0]));
        assert!(TimelineLevel::Info.keeps_sticks(Some([10, 0, 0, 0]), [0; 4]));
    }

    #[test]
    fn config_text_picks_the_level() {
        assert_eq!(TimelineLevel::parse(""), Ok(TimelineLevel::Info));
        assert_eq!(
            TimelineLevel::parse("timeline = \"debug\""),
            Ok(TimelineLevel::Debug)
        );
        assert_eq!(
            TimelineLevel::parse("timeline = \"info\""),
            Ok(TimelineLevel::Info)
        );
        assert!(TimelineLevel::parse("timeline = \"loud\"").is_err());
    }
}
