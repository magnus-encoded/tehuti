//! Nonblocking, bounded speech process transport.

use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::process::CommandExt,
    process::{Child, ChildStdin, ChildStdout, Stdio},
};
use tehuti_core::speech::{SpeechEffect, SpeechFailure, SpeechInput};
use tehuti_protocol::{MAX_MESSAGE_BYTES, Text};

#[derive(Debug)]
struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    output: VecDeque<u8>,
    input: Vec<u8>,
    ready: bool,
    device: Option<SpeechDevice>,
    vocabulary: Option<Vec<u8>>,
    age: core::time::Duration,
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
    restart: crate::child_process::Restart,
    outage: bool,
    notices: VecDeque<&'static str>,
    vocabulary: Vec<u8>,
}

impl SpeechProcess {
    #[must_use]
    pub fn new(command: Vec<OsString>) -> Self {
        Self {
            command,
            running: None,
            restart: crate::child_process::Restart::default(),
            outage: false,
            notices: VecDeque::new(),
            vocabulary: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.ready)
    }

    /// The device and compute type the running sidecar chose.
    #[must_use]
    pub fn device(&self) -> Option<&SpeechDevice> {
        self.running.as_ref().and_then(|running| running.device.as_ref())
    }

    fn start(&self) -> io::Result<Running> {
        let (program, arguments) = self
            .command
            .split_first()
            .ok_or_else(|| io::Error::other("empty speech command"))?;
        let mut child = crate::child_process::command(program)
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
                device: None,
                vocabulary: (!self.vocabulary.is_empty()).then(|| self.vocabulary.clone()),
                age: core::time::Duration::ZERO,
            }),
            Err(error) => {
                terminate(&mut child);
                Err(error)
            }
        }
    }

    /// Replaces the vocabulary and replays it to each restarted sidecar.
    ///
    /// # Errors
    /// The encoded vocabulary exceeds half the bounded output queue.
    pub fn set_vocabulary(&mut self, words: &[String]) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&serde_json::json!({"vocabulary": words}))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_MESSAGE_BYTES / 2 {
            return Err(io::Error::other("speech vocabulary exceeds limit"));
        }
        if bytes != self.vocabulary {
            if let Some(running) = &mut self.running {
                running.vocabulary = Some(bytes.clone());
            }
            self.vocabulary = bytes;
        }
        Ok(())
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
        let bytes = serde_json::to_vec(&value)?;
        let running = self
            .running
            .as_mut()
            .ok_or_else(|| io::Error::other("speech unavailable"))?;
        running.queue_vocabulary()?;
        if running.output.len().saturating_add(bytes.len()).saturating_add(1) > MAX_MESSAGE_BYTES {
            return Err(io::Error::other("speech output queue full"));
        }
        running.output.extend(bytes);
        running.output.push_back(b'\n');
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
        self.restart.failed();
    }

    /// Polls bounded pipe work and supervises children with capped restart backoff.
    pub fn poll(&mut self, elapsed: core::time::Duration) -> Vec<SpeechInput> {
        self.restart.advance(elapsed, self.running.is_some());
        if self.running.is_none() && self.restart.ready() {
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
        match running.poll() {
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
    fn queue_vocabulary(&mut self) -> io::Result<()> {
        if let Some(bytes) = &self.vocabulary {
            if self.output.len().saturating_add(bytes.len()) > MAX_MESSAGE_BYTES {
                return Err(io::Error::other("speech output queue full"));
            }
            self.output.extend(bytes);
            self.vocabulary = None;
        }
        Ok(())
    }

    fn poll(&mut self) -> io::Result<Vec<SpeechInput>> {
        if let Some(status) = self.child.try_wait()? {
            tracing::warn!(%status, "speech sidecar exited");
            return Err(io::Error::other(format!("speech exited: {status}")));
        }
        if !self.ready && self.age >= core::time::Duration::from_secs(120) {
            return Err(io::Error::other("speech readiness timed out"));
        }
        let _queued = self.queue_vocabulary();
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
                                    self.device = parse_ready_device(&self.input);
                                    tracing::info!(device = ?self.device, "speech sidecar ready");
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

/// Where the sidecar runs inference, as reported in its ready reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechDevice {
    device: String,
    compute_type: String,
}

impl SpeechDevice {
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} / {}", self.device, self.compute_type).to_uppercase()
    }
}

/// The Input page's speech line: off, starting, or the chosen device.
#[must_use]
pub fn speech_label(process: Option<&SpeechProcess>) -> String {
    let Some(process) = process else {
        return "OFF".to_owned();
    };
    process.device().map_or_else(
        || if process.is_ready() { "UNKNOWN" } else { "STARTING" }.to_owned(),
        SpeechDevice::label,
    )
}

fn device_token(value: &serde_json::Value) -> Option<String> {
    let token = value.as_str()?;
    (!token.is_empty()
        && token.len() <= 24
        && token
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    .then(|| token.to_owned())
}

fn parse_ready_device(bytes: &[u8]) -> Option<SpeechDevice> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    Some(SpeechDevice {
        device: device_token(value.get("device")?)?,
        compute_type: device_token(value.get("compute_type")?)?,
    })
}

fn parse_reply(bytes: &[u8]) -> Option<SpeechInput> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let fields = value.as_object()?;
    let ready_fields = if fields.contains_key("device") && fields.contains_key("compute_type") {
        3
    } else {
        1
    };
    if fields.len() == ready_fields && value.get("ready").and_then(serde_json::Value::as_bool) == Some(true) {
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
    fn ready_reply_names_the_device_shown_in_settings() {
        let reply = br#"{"ready":true,"device":"cuda","compute_type":"int8_float16"}"#;
        assert_eq!(parse_reply(reply), Some(SpeechInput::Available));
        assert_eq!(
            parse_ready_device(reply).map(|device| device.label()),
            Some("CUDA / INT8_FLOAT16".to_owned())
        );
        assert_eq!(parse_ready_device(br#"{"ready":true}"#), None);
    }

    #[test]
    fn ready_reply_with_an_invalid_device_is_ready_without_one() {
        let reply = br#"{"ready":true,"device":"cuda; rm","compute_type":"int8"}"#;
        assert_eq!(parse_reply(reply), Some(SpeechInput::Available));
        assert_eq!(parse_ready_device(reply), None);
        assert_eq!(parse_reply(br#"{"ready":true,"device":"cpu"}"#), None);
    }

    #[test]
    fn speech_label_reports_off_starting_and_the_chosen_device() {
        assert_eq!(speech_label(None), "OFF");
        let process = SpeechProcess::new(vec!["true".into()]);
        assert_eq!(speech_label(Some(&process)), "STARTING");
    }

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
