//! Audio commands run away from the compositor event loop.

use core::time::Duration;
use std::{
    io,
    process::{Command, Stdio},
    sync::mpsc,
    time::Instant,
};

use tehuti_core::sound::{AudioOutput, match_output, next_output};

const DEFAULT_SINK: &str = "@DEFAULT_AUDIO_SINK@";
const DEFAULT_SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Read,
    Set(u8),
    SetMute(bool),
    ToggleMute,
    SetMicrophoneMute(bool),
    ToggleMicrophoneMute,
    NextOutput,
    SelectOutput(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Read(Result<u8, String>),
    Set(Result<(), String>),
    Mute(Result<bool, String>),
    MicrophoneMute(Result<bool, String>),
    OutputSelected(Result<AudioOutput, String>),
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
                let reply = handle_request(&mut run, request);
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

fn handle_request(run: &mut impl FnMut(&[String]) -> Result<String, String>, request: Request) -> Reply {
    match request {
        Request::Read => {
            Reply::Read(run(&["get-volume".into(), DEFAULT_SINK.into()]).and_then(|text| parse_volume(&text)))
        }
        Request::Set(value) => Reply::Set(
            run(&[
                "set-volume".into(),
                DEFAULT_SINK.into(),
                format!("{}%", value.min(100)),
            ])
            .map(|_| ()),
        ),
        Request::SetMute(muted) => Reply::Mute(set_mute(run, DEFAULT_SINK, if muted { "1" } else { "0" })),
        Request::ToggleMute => Reply::Mute(set_mute(run, DEFAULT_SINK, "toggle")),
        Request::SetMicrophoneMute(muted) => {
            Reply::MicrophoneMute(set_mute(run, DEFAULT_SOURCE, if muted { "1" } else { "0" }))
        }
        Request::ToggleMicrophoneMute => Reply::MicrophoneMute(set_mute(run, DEFAULT_SOURCE, "toggle")),
        Request::NextOutput => Reply::OutputSelected(switch_output(run, None)),
        Request::SelectOutput(query) => Reply::OutputSelected(switch_output(run, Some(&query))),
    }
}

fn set_mute(
    run: &mut impl FnMut(&[String]) -> Result<String, String>,
    target: &str,
    mode: &str,
) -> Result<bool, String> {
    let _applied = run(&["set-mute".into(), target.into(), mode.into()])?;
    let reply = run(&["get-volume".into(), target.into()])?;
    parse_muted(&reply)
}

fn switch_output(
    run: &mut impl FnMut(&[String]) -> Result<String, String>,
    query: Option<&str>,
) -> Result<AudioOutput, String> {
    let status = run(&["status".into()])?;
    let sinks = parse_sinks(&status);
    let chosen = match query {
        Some(name) => match_output(&sinks, name).ok_or_else(|| "No matching speaker".to_owned())?,
        None => next_output(&sinks).ok_or_else(|| "No audio output".to_owned())?,
    };
    let _applied = run(&["set-default".into(), chosen.id.to_string()])?;
    Ok(AudioOutput {
        default: true,
        ..chosen.clone()
    })
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

fn parse_muted(text: &str) -> Result<bool, String> {
    let _volume = parse_volume(text)?;
    Ok(text.split_whitespace().any(|token| token == "[MUTED]"))
}

/// Parses the `Audio` -> `Sinks:` section of `wpctl status` output.
#[must_use]
pub fn parse_sinks(text: &str) -> Vec<AudioOutput> {
    let mut in_audio = false;
    let mut in_sinks = false;
    let mut sinks = Vec::new();
    for raw_line in text.lines() {
        if raw_line.starts_with(|character: char| character.is_ascii_alphabetic()) {
            in_audio = raw_line.trim() == "Audio";
            in_sinks = false;
            continue;
        }
        if !in_audio {
            continue;
        }
        let content = raw_line
            .trim_start_matches(|character: char| {
                character.is_whitespace() || matches!(character, '│' | '├' | '└' | '─' | '|' | '+' | '-')
            })
            .trim();
        if content == "Sinks:" {
            in_sinks = true;
        } else if content.ends_with(':') {
            in_sinks = false;
        } else if in_sinks && let Some(sink) = parse_sink_line(content) {
            sinks.push(sink);
        }
    }
    sinks
}

fn parse_sink_line(content: &str) -> Option<AudioOutput> {
    let (default, rest) = content
        .strip_prefix('*')
        .map_or((false, content), |after| (true, after.trim_start()));
    let (id_text, after_dot) = rest.split_once('.')?;
    let id = id_text.trim().parse::<u32>().ok()?;
    let name = strip_volume_suffix(after_dot).trim();
    (!name.is_empty()).then(|| AudioOutput {
        id,
        name: name.into(),
        default,
    })
}

fn strip_volume_suffix(text: &str) -> &str {
    let trimmed = text.trim_end();
    if trimmed.ends_with(']')
        && let Some((head, _tail)) = trimmed.rsplit_once("[vol:")
    {
        return head;
    }
    trimmed
}
