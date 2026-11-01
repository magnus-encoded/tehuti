//! Audio commands run away from the compositor event loop.

use core::time::Duration;
use std::{
    io,
    process::{Command, Stdio},
    sync::mpsc,
    time::Instant,
};

#[derive(Debug, Clone, Copy)]
pub enum Request {
    Read,
    Set(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Read(Result<u8, String>),
    Set(Result<(), String>),
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
        Self::with_command(command)
    }

    /// # Errors
    /// The worker thread could not be started.
    pub fn with_command(
        mut run: impl FnMut(&[String]) -> Result<String, String> + Send + 'static,
    ) -> io::Result<Self> {
        let (requests, input) = mpsc::channel();
        let (output, replies) = mpsc::channel();
        let _worker = std::thread::Builder::new().name("volume".into()).spawn(move || {
            while let Ok(request) = input.recv() {
                let reply = match request {
                    Request::Read => Reply::Read(
                        run(&["get-volume".into(), "@DEFAULT_AUDIO_SINK@".into()])
                            .and_then(|text| parse_volume(&text)),
                    ),
                    Request::Set(value) => Reply::Set(
                        run(&[
                            "set-volume".into(),
                            "@DEFAULT_AUDIO_SINK@".into(),
                            format!("{}%", value.min(100)),
                        ])
                        .map(|_| ()),
                    ),
                };
                if output.send(reply).is_err() {
                    break;
                }
            }
        })?;
        Ok(Self { requests, replies })
    }

    /// # Errors
    /// The audio worker exited.
    pub fn request(&self, request: Request) -> Result<(), String> {
        self.requests.send(request).map_err(|error| error.to_string())
    }

    pub fn poll(&self) -> impl Iterator<Item = Reply> + '_ {
        self.replies.try_iter()
    }
}

fn command(args: &[String]) -> Result<String, String> {
    let mut child = Command::new("wpctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_millis(500) => {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => {
                let _killed = child.kill();
                let _reaped = child.wait();
                return Err(result
                    .err()
                    .map_or_else(|| "audio command timed out".into(), |error| error.to_string()));
            }
        }
    }
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("audio command failed".into());
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

fn parse_volume(text: &str) -> Result<u8, String> {
    let number = text
        .strip_prefix("Volume: ")
        .and_then(|text| text.split_whitespace().next())
        .ok_or("invalid volume reply")?;
    let (whole, fraction) = number.split_once('.').ok_or("invalid volume number")?;
    if whole.is_empty()
        || fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err("invalid volume number".into());
    }
    let whole: u32 = whole.parse().map_err(|_| "invalid volume number")?;
    let digits = format!("{fraction}00");
    let percent: u32 = digits
        .get(..2)
        .ok_or("invalid volume precision")?
        .parse()
        .map_err(|_| "invalid volume precision")?;
    Ok(u8::try_from(whole.saturating_mul(100).saturating_add(percent).min(100)).unwrap_or(100))
}
