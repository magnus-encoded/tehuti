//! Hardware brightness control (sysfs backlight with DDC fallback) run away from the event loop.

use core::fmt;
use core::time::Duration;
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::Instant,
};
use tehuti_core::adjuster::{BRIGHTNESS_MAX, BRIGHTNESS_MIN, clamp_brightness};

const SYSFS_BACKLIGHT_ROOT: &str = "/sys/class/backlight";
const DDC_TIMEOUT: Duration = Duration::from_secs(3);
const BRIGHTNESSCTL_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Read,
    Set(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No sysfs backlight device or DDC display was found.
    NoDevice,
    /// A brightness read or write failed on an available backend.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => formatter.write_str("no backlight device"),
            Self::Failed(reason) => formatter.write_str(reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Read(Result<u8, Error>),
    Set(Result<(), Error>),
}

#[derive(Debug)]
pub struct Backend {
    requests: mpsc::Sender<Request>,
    replies: mpsc::Receiver<Reply>,
}

impl Backend {
    /// # Errors
    /// The worker thread could not be started.
    pub fn start() -> io::Result<Self> {
        Self::with_drivers(PathBuf::from(SYSFS_BACKLIGHT_ROOT), run_ddc, run_brightnessctl)
    }

    /// Starts a worker backed by a custom sysfs directory and DDC runner, for tests.
    ///
    /// # Errors
    /// The worker thread could not be started.
    pub fn with_sysfs_and_ddc(
        sysfs_root: PathBuf,
        run_ddc: impl FnMut(&[String]) -> Result<String, Error> + Send + 'static,
    ) -> io::Result<Self> {
        Self::with_drivers(sysfs_root, run_ddc, |_, _| {
            Err(Error::Failed("permission denied writing backlight".into()))
        })
    }

    fn with_drivers(
        sysfs_root: PathBuf,
        mut ddc: impl FnMut(&[String]) -> Result<String, Error> + Send + 'static,
        mut permission_fallback: impl FnMut(&str, u8) -> Result<(), Error> + Send + 'static,
    ) -> io::Result<Self> {
        let (requests, input) = mpsc::channel();
        let (output, replies) = mpsc::channel();
        let _worker = std::thread::Builder::new()
            .name("brightness".into())
            .spawn(move || {
                let mut ddc_max: Option<u64> = None;
                while let Ok(request) = input.recv() {
                    let reply = handle_request(
                        request,
                        &sysfs_root,
                        &mut ddc_max,
                        &mut ddc,
                        &mut permission_fallback,
                    );
                    if output.send(reply).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { requests, replies })
    }

    /// # Errors
    /// The brightness worker exited.
    pub fn request(&self, request: Request) -> Result<(), String> {
        self.requests.send(request).map_err(|error| error.to_string())
    }

    pub fn poll(&self) -> impl Iterator<Item = Reply> + '_ {
        self.replies.try_iter()
    }
}

fn handle_request(
    request: Request,
    sysfs_root: &Path,
    ddc_max: &mut Option<u64>,
    ddc: &mut dyn FnMut(&[String]) -> Result<String, Error>,
    permission_fallback: &mut dyn FnMut(&str, u8) -> Result<(), Error>,
) -> Reply {
    match request {
        Request::Read => Reply::Read(read_brightness(sysfs_root, ddc_max, ddc)),
        Request::Set(value) => Reply::Set(write_brightness(
            sysfs_root,
            clamp_brightness(value),
            *ddc_max,
            ddc,
            permission_fallback,
        )),
    }
}

fn read_brightness(
    sysfs_root: &Path,
    ddc_max: &mut Option<u64>,
    ddc: &mut dyn FnMut(&[String]) -> Result<String, Error>,
) -> Result<u8, Error> {
    if let Some(device) = find_sysfs_device(sysfs_root) {
        return read_sysfs(&device);
    }
    let output = ddc(&["getvcp".into(), "10".into(), "--terse".into()])?;
    let (current, max) = parse_ddc_terse(&output)?;
    *ddc_max = Some(max);
    Ok(raw_to_percent(current, max))
}

fn write_brightness(
    sysfs_root: &Path,
    percent: u8,
    ddc_max: Option<u64>,
    ddc: &mut dyn FnMut(&[String]) -> Result<String, Error>,
    permission_fallback: &mut dyn FnMut(&str, u8) -> Result<(), Error>,
) -> Result<(), Error> {
    if let Some(device) = find_sysfs_device(sysfs_root) {
        return write_sysfs(&device, percent, permission_fallback);
    }
    let max = ddc_max.unwrap_or(100);
    let target = percent_to_raw(percent, max);
    let _output = ddc(&["setvcp".into(), "10".into(), target.to_string()])?;
    Ok(())
}

/// Converts a raw sysfs or DDC reading into a clamped percentage (`1..=100`).
#[must_use]
pub fn raw_to_percent(raw: u64, max: u64) -> u8 {
    if max == 0 {
        return BRIGHTNESS_MIN;
    }
    let clamped = raw.min(max);
    let rounded = u128::from(clamped)
        .saturating_mul(100)
        .saturating_add(u128::from(max / 2))
        .checked_div(u128::from(max))
        .unwrap_or_default();
    let percent = u8::try_from(rounded).unwrap_or(BRIGHTNESS_MAX);
    clamp_brightness(percent)
}

/// Converts a brightness percentage (`1..=100`) into a raw value in `1..=max` (or `0` when `max == 0`).
#[must_use]
pub fn percent_to_raw(percent: u8, max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let clamped = clamp_brightness(percent);
    let rounded = u128::from(clamped)
        .saturating_mul(u128::from(max))
        .saturating_add(50)
        .checked_div(100)
        .unwrap_or_default();
    u64::try_from(rounded).unwrap_or(max).clamp(1, max)
}

/// Parses `ddcutil getvcp 10 --terse` output (`VCP 10 C <current> <max>`).
///
/// # Errors
/// Returns [`Error::Failed`] when no valid continuous VCP 10 line is present.
pub fn parse_ddc_terse(text: &str) -> Result<(u64, u64), Error> {
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if let [vcp, code, kind, current, max, ..] = tokens.as_slice()
            && *vcp == "VCP"
            && *code == "10"
            && *kind == "C"
        {
            let current: u64 = current
                .parse()
                .map_err(|_| Error::Failed("invalid DDC brightness value".into()))?;
            let max: u64 = max
                .parse()
                .map_err(|_| Error::Failed("invalid DDC maximum value".into()))?;
            if max == 0 {
                return Err(Error::Failed("DDC maximum brightness is zero".into()));
            }
            return Ok((current, max));
        }
    }
    Err(Error::Failed("invalid DDC brightness reply".into()))
}

struct SysfsDevice {
    name: String,
    brightness_path: PathBuf,
    actual_path: PathBuf,
    max: u64,
    rank: u8,
}

fn find_sysfs_device(root: &Path) -> Option<SysfsDevice> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut best: Option<SysfsDevice> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(max) = read_u64(&path.join("max_brightness")) else {
            continue;
        };
        if max == 0 {
            continue;
        }
        let brightness_path = path.join("brightness");
        let actual_path = path.join("actual_brightness");
        if !brightness_path.exists() && !actual_path.exists() {
            continue;
        }
        let rank = sysfs_type_rank(&path.join("type"));
        let candidate = SysfsDevice {
            name,
            brightness_path,
            actual_path,
            max,
            rank,
        };
        let replace = best.as_ref().is_none_or(|chosen| {
            candidate.rank < chosen.rank || (candidate.rank == chosen.rank && candidate.name < chosen.name)
        });
        if replace {
            best = Some(candidate);
        }
    }
    best
}

fn sysfs_type_rank(path: &Path) -> u8 {
    match std::fs::read_to_string(path).ok().as_deref().map(str::trim) {
        Some("raw") => 0,
        Some("platform") => 1,
        Some("firmware") => 2,
        _ => 3,
    }
}

fn read_u64(path: &Path) -> Result<u64, Error> {
    let text = std::fs::read_to_string(path).map_err(|error| Error::Failed(error.to_string()))?;
    text.trim()
        .parse::<u64>()
        .map_err(|_| Error::Failed("invalid backlight value".into()))
}

fn read_sysfs(device: &SysfsDevice) -> Result<u8, Error> {
    let raw = read_u64(&device.actual_path).or_else(|_| read_u64(&device.brightness_path))?;
    Ok(raw_to_percent(raw, device.max))
}

fn write_sysfs(
    device: &SysfsDevice,
    percent: u8,
    permission_fallback: &mut dyn FnMut(&str, u8) -> Result<(), Error>,
) -> Result<(), Error> {
    let raw = percent_to_raw(percent, device.max);
    match std::fs::write(&device.brightness_path, raw.to_string()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            permission_fallback(&device.name, percent)
        }
        Err(error) => Err(Error::Failed(error.to_string())),
    }
}

fn run_command(program: &str, args: &[String], timeout: Duration) -> Result<Output, Error> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::NoDevice
            } else {
                Error::Failed(error.to_string())
            }
        })?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => {
                let _killed = child.kill();
                let _reaped = child.wait();
                return Err(Error::Failed(result.err().map_or_else(
                    || "brightness command timed out".into(),
                    |error| error.to_string(),
                )));
            }
        }
    }
    child
        .wait_with_output()
        .map_err(|error| Error::Failed(error.to_string()))
}

fn run_ddc(args: &[String]) -> Result<String, Error> {
    let output = run_command("ddcutil", args, DDC_TIMEOUT)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        if stderr.contains("not found") || stderr.contains("no display") || stderr.contains("cannot find") {
            return Err(Error::NoDevice);
        }
        return Err(Error::Failed("DDC brightness command failed".into()));
    }
    String::from_utf8(output.stdout).map_err(|error| Error::Failed(error.to_string()))
}

fn run_brightnessctl(device: &str, percent: u8) -> Result<(), Error> {
    let args = [
        "--device".into(),
        device.into(),
        "set".into(),
        format!("{percent}%"),
    ];
    let output = run_command("brightnessctl", &args, BRIGHTNESSCTL_TIMEOUT).map_err(|error| match error {
        Error::NoDevice => Error::Failed("permission denied writing backlight".into()),
        Error::Failed(reason) => Error::Failed(reason),
    })?;
    if !output.status.success() {
        return Err(Error::Failed("brightnessctl failed".into()));
    }
    Ok(())
}
