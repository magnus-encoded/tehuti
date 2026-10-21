//! Nonblocking, bounded speech process transport.

use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::process::CommandExt,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
use tehuti_core::speech::{SpeechEffect, SpeechFailure, SpeechInput};
use tehuti_protocol::{MAX_MESSAGE_BYTES, Text};

const CAPTURE_TAIL: core::time::Duration = core::time::Duration::from_millis(300);

#[derive(Debug)]
struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    output: VecDeque<u8>,
    input: Vec<u8>,
    ready: bool,
    age: core::time::Duration,
    tail: Option<(u64, core::time::Duration)>,
}

impl Drop for Running {
    fn drop(&mut self) {
        terminate(&mut self.child);
    }
}

fn terminate(child: &mut Child) {
    crate::shutdown::terminate(child, true);
}

#[derive(Debug)]
pub struct SpeechProcess {
    command: Vec<OsString>,
    running: Option<Running>,
    retry: core::time::Duration,
    failures: usize,
    outage: bool,
    notices: VecDeque<&'static str>,
}

impl SpeechProcess {
    #[must_use]
    pub const fn new(command: Vec<OsString>) -> Self {
        Self {
            command,
            running: None,
            retry: core::time::Duration::ZERO,
            failures: 0,
            outage: false,
            notices: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.ready)
    }

    fn start(&self) -> io::Result<Running> {
        let (program, arguments) = self
            .command
            .split_first()
            .ok_or_else(|| io::Error::other("empty speech command"))?;
        let mut child = Command::new(program)
            .args(arguments)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let pipes = (|| {
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("speech stdin missing"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("speech stdout missing"))?;
            rustix::fs::fcntl_setfl(&stdin, rustix::fs::OFlags::NONBLOCK)?;
            rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)?;
            Ok((stdin, stdout))
        })();
        match pipes {
            Ok((stdin, stdout)) => Ok(Running {
                child,
                stdin,
                stdout,
                output: VecDeque::new(),
                input: Vec::new(),
                ready: false,
                age: core::time::Duration::ZERO,
                tail: None,
            }),
            Err(error) => {
                terminate(&mut child);
                Err(error)
            }
        }
    }

    /// Queues a capture command without waiting for the child.
    ///
    /// # Errors
    /// The child is absent or its bounded output queue is full.
    pub fn send(&mut self, effect: &SpeechEffect) -> io::Result<()> {
        let value = match effect {
            SpeechEffect::Start { session } => serde_json::json!({"start": session}),
            SpeechEffect::Stop { session, transcribe } => {
                serde_json::json!({"stop": session, "transcribe": transcribe})
            }
            SpeechEffect::Interim { .. } | SpeechEffect::Speech { .. } | SpeechEffect::Failed { .. } => {
                return Ok(());
            }
        };
        let running = self
            .running
            .as_mut()
            .ok_or_else(|| io::Error::other("speech unavailable"))?;
        if let SpeechEffect::Stop {
            session,
            transcribe: true,
        } = effect
        {
            running.tail = Some((*session, CAPTURE_TAIL));
            return Ok(());
        }
        let mut bytes = Vec::new();
        if let Some((session, _)) = running.tail {
            bytes.extend(serde_json::to_vec(
                &serde_json::json!({"stop": session, "transcribe": false}),
            )?);
            bytes.push(b'\n');
        }
        bytes.extend(serde_json::to_vec(&value)?);
        bytes.push(b'\n');
        running.queue(&bytes)?;
        running.tail = None;
        Ok(())
    }

    /// Takes the next outage or recovery notice for the compositor notice path.
    pub fn take_notice(&mut self) -> Option<&'static str> {
        self.notices.pop_front()
    }

    fn failed(&mut self) {
        if !self.outage {
            self.outage = true;
            self.notices.push_back("Speech is down, restarting");
        }
        let seconds = [0, 1, 2, 5, 10, 20].get(self.failures).copied().unwrap_or(30);
        self.retry = core::time::Duration::from_secs(seconds);
        self.failures = self.failures.saturating_add(1);
    }

    /// Polls bounded pipe work and supervises children with capped restart backoff.
    pub fn poll(&mut self, elapsed: core::time::Duration) -> Vec<SpeechInput> {
        self.retry = self.retry.saturating_sub(elapsed);
        if self.running.is_none() && self.retry.is_zero() {
            match self.start() {
                Ok(running) => {
                    self.running = Some(running);
                    return Vec::new();
                }
                Err(error) => {
                    tracing::warn!(%error, "speech start failed");
                    self.failed();
                    return vec![SpeechInput::Unavailable];
                }
            }
        }
        let Some(running) = self.running.as_mut() else {
            return Vec::new();
        };
        running.age = running.age.saturating_add(elapsed);
        if running.age >= core::time::Duration::from_secs(60) {
            self.failures = 0;
        }
        match running.poll(elapsed) {
            Ok(inputs) => {
                if self.outage && running.ready {
                    self.outage = false;
                    self.notices.push_back("Speech is back");
                }
                inputs
            }
            Err(error) => {
                tracing::warn!(%error, "speech process failed");
                self.running = None;
                self.failed();
                vec![SpeechInput::Unavailable]
            }
        }
    }
}

impl Running {
    fn queue(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.output.len().saturating_add(bytes.len()) > MAX_MESSAGE_BYTES {
            return Err(io::Error::other("speech output queue full"));
        }
        self.output.extend(bytes);
        Ok(())
    }

    fn advance_tail(&mut self, elapsed: core::time::Duration) -> io::Result<()> {
        if let Some((session, remaining)) = self.tail {
            let remaining = remaining.saturating_sub(elapsed);
            if remaining.is_zero() {
                let mut bytes =
                    serde_json::to_vec(&serde_json::json!({"stop": session, "transcribe": true}))?;
                bytes.push(b'\n');
                self.queue(&bytes)?;
                self.tail = None;
            } else {
                self.tail = Some((session, remaining));
            }
        }
        Ok(())
    }

    fn poll(&mut self, elapsed: core::time::Duration) -> io::Result<Vec<SpeechInput>> {
        self.advance_tail(elapsed)?;
        if let Some(status) = self.child.try_wait()? {
            tracing::warn!(%status, "speech sidecar exited");
            return Err(io::Error::other(format!("speech exited: {status}")));
        }
        if !self.ready && self.age >= core::time::Duration::from_secs(120) {
            return Err(io::Error::other("speech readiness timed out"));
        }
        for _attempt in 0..16 {
            let (bytes, _) = self.output.as_slices();
            if bytes.is_empty() {
                break;
            }
            match self.stdin.write(bytes) {
                Ok(0) => return Err(io::Error::other("speech stdin closed")),
                Ok(count) => {
                    let _written = self.output.drain(..count);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        let mut inputs = Vec::new();
        let mut bytes = [0; 4096];
        for _attempt in 0..16 {
            match self.stdout.read(&mut bytes) {
                Ok(0) => return Err(io::Error::other("speech stdout closed")),
                Ok(count) => {
                    for byte in bytes.get(..count).unwrap_or_default() {
                        if *byte == b'\n' {
                            if let Some(input) = parse_reply(&self.input) {
                                if input == SpeechInput::Available {
                                    tracing::info!("speech sidecar ready");
                                    self.ready = true;
                                }
                                inputs.push(input);
                            } else {
                                tracing::warn!(
                                    reply = %String::from_utf8_lossy(&self.input),
                                    "speech reply ignored: unrecognized"
                                );
                            }
                            self.input.clear();
                        } else {
                            if self.input.len() >= MAX_MESSAGE_BYTES {
                                return Err(io::Error::other("speech line exceeds limit"));
                            }
                            self.input.push(*byte);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(inputs)
    }
}

fn parse_reply(bytes: &[u8]) -> Option<SpeechInput> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let fields = value.as_object()?;
    if fields.len() == 1 && value.get("ready").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some(SpeechInput::Available);
    }
    if fields.len() != 2 {
        return None;
    }
    let session = value.get("session")?.as_u64()?;
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        let reason = if error == "silent" {
            SpeechFailure::Silent
        } else {
            SpeechFailure::Transcription
        };
        return Some(SpeechInput::Failed { session, reason });
    }
    if let Some(text) = value.get("interim").and_then(serde_json::Value::as_str) {
        return Some(SpeechInput::Interim {
            session,
            text: Text::new(text.to_owned()).ok()?,
        });
    }
    let text = Text::new(value.get("text")?.as_str()?.to_owned()).ok()?;
    Some(SpeechInput::Transcript { session, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interim_reply_preserves_text_and_rejects_invalid_boundaries() {
        assert_eq!(
            parse_reply(br#"{"session":7,"interim":"Live Words"}"#),
            Some(SpeechInput::Interim {
                session: 7,
                text: Text::new("Live Words".into()).unwrap()
            })
        );
        assert_eq!(parse_reply(br#"{"session":7,"interim":"bad\u0000"}"#), None);
        assert_eq!(parse_reply(br#"{"session":7,"interim":"hi","extra":true}"#), None);
    }

    #[test]
    fn sidecar_error_reply_is_a_failed_transcript() {
        assert_eq!(
            parse_reply(br#"{"session": 4, "error": "boom"}"#),
            Some(SpeechInput::Failed {
                session: 4,
                reason: SpeechFailure::Transcription
            })
        );
    }

    #[test]
    fn sidecar_silent_reply_is_a_silent_failure() {
        assert_eq!(
            parse_reply(br#"{"session": 5, "error": "silent"}"#),
            Some(SpeechInput::Failed {
                session: 5,
                reason: SpeechFailure::Silent
            })
        );
    }
}
