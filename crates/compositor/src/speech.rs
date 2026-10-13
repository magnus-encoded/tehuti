//! Nonblocking, bounded speech process transport.

use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::process::CommandExt,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
use tehuti_core::speech::{SpeechEffect, SpeechInput};
use tehuti_protocol::{MAX_MESSAGE_BYTES, Text};

#[derive(Debug)]
struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    output: VecDeque<u8>,
    input: Vec<u8>,
    ready: bool,
    age: core::time::Duration,
}

impl Drop for Running {
    fn drop(&mut self) {
        terminate(&mut self.child);
    }
}

fn terminate(child: &mut Child) {
    if let Ok(raw_pid) = i32::try_from(child.id())
        && let Some(group) = rustix::process::Pid::from_raw(raw_pid)
        && let Err(error) = rustix::process::kill_process_group(group, rustix::process::Signal::KILL)
    {
        tracing::debug!(%error, "speech process group kill failed");
    }
    if let Err(error) = child.kill() {
        tracing::debug!(%error, "speech kill failed");
    }
    if let Err(error) = child.wait() {
        tracing::debug!(%error, "speech reap failed");
    }
}

#[derive(Debug)]
pub struct SpeechProcess {
    command: Vec<OsString>,
    running: Option<Running>,
    retry: core::time::Duration,
}

impl SpeechProcess {
    #[must_use]
    pub const fn new(command: Vec<OsString>) -> Self {
        Self {
            command,
            running: None,
            retry: core::time::Duration::ZERO,
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
            SpeechEffect::Speech { .. } | SpeechEffect::Failed { .. } => return Ok(()),
        };
        let bytes = serde_json::to_vec(&value)?;
        let running = self
            .running
            .as_mut()
            .ok_or_else(|| io::Error::other("speech unavailable"))?;
        if running.output.len().saturating_add(bytes.len()).saturating_add(1) > MAX_MESSAGE_BYTES {
            return Err(io::Error::other("speech output queue full"));
        }
        running.output.extend(bytes);
        running.output.push_back(b'\n');
        Ok(())
    }

    /// Polls bounded pipe work and restarts failed children after one second.
    pub fn poll(&mut self, elapsed: core::time::Duration) -> Vec<SpeechInput> {
        self.retry = self.retry.saturating_sub(elapsed);
        if self.running.is_none() && self.retry.is_zero() {
            match self.start() {
                Ok(running) => self.running = Some(running),
                Err(error) => {
                    tracing::warn!(%error, "speech start failed");
                    self.retry = core::time::Duration::from_secs(1);
                }
            }
        }
        let Some(running) = self.running.as_mut() else {
            return Vec::new();
        };
        running.age = running.age.saturating_add(elapsed);
        match running.poll() {
            Ok(inputs) => inputs,
            Err(error) => {
                tracing::warn!(%error, "speech process failed");
                self.running = None;
                self.retry = core::time::Duration::from_secs(1);
                vec![SpeechInput::Unavailable]
            }
        }
    }
}

impl Running {
    fn poll(&mut self) -> io::Result<Vec<SpeechInput>> {
        if self.child.try_wait()?.is_some() {
            return Err(io::Error::other("speech exited"));
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
                                    self.ready = true;
                                }
                                inputs.push(input);
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
    if value.get("error").is_some_and(serde_json::Value::is_string) {
        return Some(SpeechInput::Failed { session });
    }
    let text = Text::new(value.get("text")?.as_str()?.to_owned()).ok()?;
    Some(SpeechInput::Transcript { session, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_error_reply_is_a_failed_transcript() {
        assert_eq!(
            parse_reply(br#"{"session": 4, "error": "boom"}"#),
            Some(SpeechInput::Failed { session: 4 })
        );
    }
}
