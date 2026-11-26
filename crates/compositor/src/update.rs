//! Background release checking, update/rollback script execution, and failed-start tracking.

use core::{fmt::Write as _, time::Duration};
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{SystemTime, UNIX_EPOCH},
};
use tehuti_core::update::ReleaseInfo;

/// Default interval between background release checks.
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_hours(6);

/// Shortest allowed check interval when configured via `TEHUTI_UPDATE_CHECK_SECONDS`.
pub const MIN_CHECK_INTERVAL: Duration = Duration::from_mins(1);

/// Uptime after which a start counts as healthy and clears earlier failure markers.
pub const HEALTHY_UPTIME: Duration = Duration::from_secs(30);

/// Recent unconfirmed starts needed before the next start offers rollback.
pub const FAILED_STARTS_THRESHOLD: usize = 2;

/// Window in seconds during which failed starts count together.
pub const FAILED_STARTS_WINDOW_SECS: u64 = 10 * 60;

const ROLLBACK_OFFER_FILE: &str = "rollback-offer";
const START_ATTEMPTS_FILE: &str = "start-attempts";

/// Paths and policy for checking, applying and rolling back releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Path to the `tehuti-update` script, when installed.
    pub script: Option<PathBuf>,
    /// Release directory holding `tehuti`, `tehuti.prev`, `*.version`, and `tehuti.skip`.
    pub lib_dir: PathBuf,
    /// State directory holding `start-attempts` and `rollback-offer`.
    pub state_dir: PathBuf,
    /// Optional explicit release channel (`latest` or `prerelease`); `None` inherits `TEHUTI_CHANNEL`.
    pub channel: Option<String>,
    /// Delay between background checks.
    pub check_interval: Duration,
}

/// Completion reported by a background update or rollback worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionResult {
    /// `tehuti-update` succeeded.
    Updated {
        installed: Option<String>,
        previous: Option<String>,
    },
    /// `tehuti-update --rollback` succeeded.
    RolledBack {
        installed: Option<String>,
        previous: Option<String>,
    },
    /// The script failed or could not be started.
    Failed { action: &'static str, reason: String },
}

/// Manages release checks, update/rollback execution and failed-start markers off the compositor loop.
#[derive(Debug)]
pub struct Manager {
    config: Config,
    check_rx: Option<Receiver<ReleaseInfo>>,
    action_rx: Option<Receiver<ActionResult>>,
    healthy_marked: bool,
}

impl Manager {
    /// Builds a manager from the session's XDG and `TEHUTI_*` environment variables.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let lib_dir = std::env::var_os("TEHUTI_LIB")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".local/lib/tehuti"), PathBuf::from);
        let state_dir = std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".local/state"), PathBuf::from)
            .join("tehuti");
        let script = discover_script(&home);
        let check_interval = std::env::var("TEHUTI_UPDATE_CHECK_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(DEFAULT_CHECK_INTERVAL, |seconds| {
                Duration::from_secs(seconds).max(MIN_CHECK_INTERVAL)
            });
        let channel = std::env::var("TEHUTI_CHANNEL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        Some(Self::new(Config {
            script,
            lib_dir,
            state_dir,
            channel,
            check_interval,
        }))
    }

    /// Builds a manager with explicit paths and script location.
    #[must_use]
    pub const fn new(config: Config) -> Self {
        Self {
            config,
            check_rx: None,
            action_rx: None,
            healthy_marked: false,
        }
    }

    /// Interval between background release checks.
    #[must_use]
    pub const fn check_interval(&self) -> Duration {
        self.config.check_interval
    }

    /// Reads local release metadata and records this start; returns `(info, offer_rollback)`.
    #[must_use]
    pub fn initial_state(&self, now_secs: u64) -> (ReleaseInfo, bool) {
        let info = read_local_release_info(&self.config.lib_dir);
        let repeated_failures = record_start(&self.config.state_dir, now_secs);
        let offer = repeated_failures && info.previous.is_some();
        (info, offer)
    }

    /// Marks the current start healthy and clears prior failed-start markers.
    pub fn mark_healthy(&mut self) {
        if !self.healthy_marked {
            clear_start_failures(&self.config.state_dir);
            self.healthy_marked = true;
        }
    }

    /// Marks the current start healthy once `uptime` reaches [`HEALTHY_UPTIME`].
    pub fn poll_healthy(&mut self, uptime: Duration) {
        if !self.healthy_marked && uptime >= HEALTHY_UPTIME {
            self.mark_healthy();
        }
    }

    /// Launches `tehuti-update --check` in the background when no check is already running.
    pub fn start_check(&mut self) {
        if self.check_rx.is_some() {
            return;
        }
        let Some(script) = self.config.script.clone() else {
            return;
        };
        let lib_dir = self.config.lib_dir.clone();
        let channel = self.config.channel.clone();
        let (sender, receiver) = mpsc::channel();
        self.check_rx = Some(receiver);
        let _worker = std::thread::spawn(move || {
            let mut command = Command::new(&script);
            let _arg = command.arg("--check");
            apply_env(&mut command, &lib_dir, channel.as_deref());
            let output = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output();
            match output {
                Ok(result) if result.status.success() => {
                    let stdout = String::from_utf8_lossy(&result.stdout);
                    let skipped = read_skipped_tag(&lib_dir);
                    let prev_ok = is_executable_file(&lib_dir.join("tehuti.prev"));
                    if let Some(info) = parse_check_output(&stdout, skipped.as_deref(), prev_ok) {
                        let _sent = sender.send(info);
                    }
                }
                Ok(result) => {
                    tracing::debug!(status = %result.status, "release check exited without a result");
                }
                Err(error) => {
                    tracing::debug!(%error, "release check could not start");
                }
            }
        });
    }

    /// Launches `tehuti-update` in the background.
    ///
    /// # Errors
    /// Returns an error if an update/rollback is already running or the script is missing.
    pub fn start_apply(&mut self) -> Result<(), String> {
        self.start_action(false)
    }

    /// Launches `tehuti-update --rollback` in the background.
    ///
    /// # Errors
    /// Returns an error if an update/rollback is already running or the script is missing.
    pub fn start_rollback(&mut self) -> Result<(), String> {
        self.start_action(true)
    }

    fn start_action(&mut self, rollback: bool) -> Result<(), String> {
        if self.action_rx.is_some() {
            return Err("ALREADY RUNNING".into());
        }
        let Some(script) = self.config.script.clone() else {
            return Err("TEHUTI-UPDATE NOT FOUND".into());
        };
        let lib_dir = self.config.lib_dir.clone();
        let state_dir = self.config.state_dir.clone();
        let channel = self.config.channel.clone();
        let (sender, receiver) = mpsc::channel();
        self.action_rx = Some(receiver);
        let _worker = std::thread::spawn(move || {
            let outcome = run_update_script(&script, &lib_dir, &state_dir, channel.as_deref(), rollback);
            let _sent = sender.send(outcome);
        });
        Ok(())
    }

    /// Returns a completed background release check, if one arrived.
    pub fn poll_check(&mut self) -> Option<ReleaseInfo> {
        let receiver = self.check_rx.as_ref()?;
        match receiver.try_recv() {
            Ok(info) => {
                self.check_rx = None;
                Some(info)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.check_rx = None;
                None
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }

    /// Returns a completed background update or rollback outcome, if one arrived.
    pub fn poll_action(&mut self) -> Option<ActionResult> {
        let receiver = self.action_rx.as_ref()?;
        match receiver.try_recv() {
            Ok(result) => {
                self.action_rx = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.action_rx = None;
                Some(ActionResult::Failed {
                    action: "UPDATE",
                    reason: "WORKER STOPPED".into(),
                })
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}

/// Current unix timestamp in seconds, or 0 if the wall clock precedes the epoch.
#[must_use]
pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Reads installed and fallback tags from `lib_dir` without touching the network.
#[must_use]
pub fn read_local_release_info(lib_dir: &Path) -> ReleaseInfo {
    let installed = read_tag_file(&lib_dir.join("tehuti.version"));
    let previous = if is_executable_file(&lib_dir.join("tehuti.prev")) {
        Some(read_tag_file(&lib_dir.join("tehuti.prev.version")).unwrap_or_else(|| "unknown".into()))
    } else {
        None
    };
    ReleaseInfo {
        installed,
        previous,
        available: None,
    }
}

/// Reads the release tag skipped after a rollback (`tehuti.skip`), if present.
#[must_use]
pub fn read_skipped_tag(lib_dir: &Path) -> Option<String> {
    read_tag_file(&lib_dir.join("tehuti.skip"))
}

/// Parses `tehuti-update --check` output into [`ReleaseInfo`].
#[must_use]
pub fn parse_check_output(stdout: &str, skipped: Option<&str>, prev_executable: bool) -> Option<ReleaseInfo> {
    let line = stdout.lines().find(|line| {
        line.contains("installed ") && line.contains(", fallback ") && line.contains(", newest ")
    })?;
    let (_, after_installed) = line.split_once("installed ")?;
    let (installed_raw, after_fallback) = after_installed.split_once(", fallback ")?;
    let (fallback_raw, after_newest) = after_fallback.split_once(", newest ")?;
    let newest_raw = after_newest.split_whitespace().next().unwrap_or_default();

    let installed = normalize_tag(installed_raw);
    let fallback = normalize_tag(fallback_raw);
    let newest = normalize_tag(newest_raw);
    let previous = if prev_executable {
        Some(fallback.unwrap_or_else(|| "unknown".into()))
    } else {
        None
    };
    let available = installed
        .as_deref()
        .zip(newest.as_deref())
        .and_then(|(current, latest)| {
            (current != latest && skipped != Some(latest)).then(|| latest.to_owned())
        });
    Some(ReleaseInfo {
        installed,
        previous,
        available,
    })
}

/// Records this start in `state_dir` and returns true when repeated failed starts warrant offering rollback.
#[must_use]
pub fn record_start(state_dir: &Path, now_secs: u64) -> bool {
    let offer_path = state_dir.join(ROLLBACK_OFFER_FILE);
    let mut offer = offer_path.exists();
    if offer {
        let _removed = fs::remove_file(&offer_path);
    }

    let attempts_path = state_dir.join(START_ATTEMPTS_FILE);
    let mut recent: Vec<u64> = fs::read_to_string(&attempts_path)
        .ok()
        .map_or_else(Vec::new, |text| {
            text.lines()
                .filter_map(|line| line.trim().parse::<u64>().ok())
                .filter(|timestamp| now_secs.saturating_sub(*timestamp) <= FAILED_STARTS_WINDOW_SECS)
                .collect()
        });

    if recent.len() >= FAILED_STARTS_THRESHOLD {
        offer = true;
    }

    recent.push(now_secs);
    if recent.len() > 8 {
        let excess = recent.len().saturating_sub(8);
        let _drained = recent.drain(..excess);
    }
    if fs::create_dir_all(state_dir).is_ok() {
        let mut body = String::new();
        for timestamp in &recent {
            let _formatted = writeln!(body, "{timestamp}");
        }
        let _written = fs::write(&attempts_path, body);
    }
    offer
}

/// Clears pending failed-start markers from `state_dir`.
pub fn clear_start_failures(state_dir: &Path) {
    let _attempts = fs::remove_file(state_dir.join(START_ATTEMPTS_FILE));
    let _offer = fs::remove_file(state_dir.join(ROLLBACK_OFFER_FILE));
}

fn discover_script(home: &Path) -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("TEHUTI_UPDATE_SCRIPT").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(configured);
        if is_executable_file(&path) {
            return Some(path);
        }
    }
    let local_bin = home.join(".local/bin/tehuti-update");
    if is_executable_file(&local_bin) {
        return Some(local_bin);
    }
    if let Some(release_dir) = std::env::var_os("TEHUTI_RELEASE_DIRECTORY").filter(|value| !value.is_empty())
    {
        for candidate in ["scripts/tehuti-update", "dist/tehuti-update"] {
            let path = PathBuf::from(&release_dir).join(candidate);
            if is_executable_file(&path) {
                return Some(path);
            }
        }
    }
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    crate::system_menu::is_installed("tehuti-update", &search_path).then(|| PathBuf::from("tehuti-update"))
}

fn run_update_script(
    script: &Path,
    lib_dir: &Path,
    state_dir: &Path,
    channel: Option<&str>,
    rollback: bool,
) -> ActionResult {
    let mut command = Command::new(script);
    if rollback {
        let _arg = command.arg("--rollback");
    }
    apply_env(&mut command, lib_dir, channel);
    let action = if rollback { "ROLLBACK" } else { "UPDATE" };
    match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) if output.status.success() => {
            if rollback {
                clear_start_failures(state_dir);
            }
            let info = read_local_release_info(lib_dir);
            if rollback {
                ActionResult::RolledBack {
                    installed: info.installed,
                    previous: info.previous,
                }
            } else {
                ActionResult::Updated {
                    installed: info.installed,
                    previous: info.previous,
                }
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let reason = first_error_line(&stderr)
                .or_else(|| first_error_line(&stdout))
                .map_or_else(|| format!("EXIT {}", output.status), str::to_owned);
            ActionResult::Failed { action, reason }
        }
        Err(error) => ActionResult::Failed {
            action,
            reason: error.to_string(),
        },
    }
}

fn apply_env(command: &mut Command, lib_dir: &Path, channel: Option<&str>) {
    let _lib = command.env("TEHUTI_LIB", lib_dir);
    if let Some(channel) = channel {
        let _channel = command.env("TEHUTI_CHANNEL", channel);
    }
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn read_tag_file(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    normalize_tag(&text)
}

fn normalize_tag(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn first_error_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_check_output_detects_newer_release_and_respects_skip() {
        let line = "channel latest: installed v0.148.1, fallback v0.148.0, newest v0.148.2 (https://example.test/tehuti.tar.gz)\n";
        let info = parse_check_output(line, None, true);
        assert_eq!(
            info,
            Some(ReleaseInfo {
                installed: Some("v0.148.1".into()),
                previous: Some("v0.148.0".into()),
                available: Some("v0.148.2".into()),
            })
        );

        let skipped = parse_check_output(line, Some("v0.148.2"), true);
        assert_eq!(
            skipped,
            Some(ReleaseInfo {
                installed: Some("v0.148.1".into()),
                previous: Some("v0.148.0".into()),
                available: None,
            })
        );

        let current_line = "channel latest: installed v0.148.2, fallback unknown, newest v0.148.2\n";
        let current = parse_check_output(current_line, None, false);
        assert_eq!(
            current,
            Some(ReleaseInfo {
                installed: Some("v0.148.2".into()),
                previous: None,
                available: None,
            })
        );
    }

    #[test]
    fn repeated_failed_starts_trigger_rollback_offer_until_cleared() -> Result<(), Box<dyn core::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let state_dir = directory.path();
        assert!(
            !record_start(state_dir, 1000),
            "first start must not offer rollback"
        );
        assert!(
            !record_start(state_dir, 1020),
            "second start must not offer rollback yet"
        );
        assert!(
            record_start(state_dir, 1040),
            "third start after two failed starts offers rollback"
        );
        clear_start_failures(state_dir);
        assert!(
            !record_start(state_dir, 1050),
            "clearing failures resets the offer"
        );
        Ok(())
    }

    #[test]
    fn session_rollback_offer_marker_is_consumed_once() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let state_dir = directory.path();
        fs::write(state_dir.join(ROLLBACK_OFFER_FILE), "1000\n")?;
        assert!(
            record_start(state_dir, 1005),
            "session rollback marker triggers rollback offer"
        );
        assert!(
            !record_start(state_dir, 1010),
            "marker is consumed so the next start does not repeat it"
        );
        Ok(())
    }
}
