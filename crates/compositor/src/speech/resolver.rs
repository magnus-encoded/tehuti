//! Asking a model which command an unmatched transcript meant.
//!
//! The request carries the transcript and the focused context's command names
//! and nothing else. It goes out only when the owner opted in and a key is set;
//! a missing key, an error or a slow reply leaves today's behaviour.

use core::time::Duration;
use std::{
    io::{self, Read as _, Write as _},
    path::PathBuf,
    process::Stdio,
    time::Instant,
};

use tehuti_core::speech::resolver::{Answer, Ranked, Turn};

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-latest";
const QUESTION: &str = "command";
const NO_COMMAND: &str = "no command fits";
const KEY_VARIABLE: &str = "TYPESAFE_API_KEY";

/// How long the model may take before the transcript dictates as before.
pub const DEADLINE: Duration = Duration::from_millis(400);

/// An API key that never prints itself.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// The key from the environment, when set and non-empty.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        std::env::var(KEY_VARIABLE).ok().and_then(Self::new)
    }

    #[must_use]
    pub fn new(value: String) -> Option<Self> {
        let printable = value
            .chars()
            .all(|character| character.is_ascii_graphic() && !matches!(character, '"' | '\\'));
        (printable && !value.is_empty()).then_some(Self(value))
    }
}

impl core::fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("ApiKey(redacted)")
    }
}

/// Sends one request body and returns the reply body within the deadline.
pub trait Transport {
    /// # Errors
    /// The request failed, was refused or ran past the deadline.
    fn post(&self, key: &ApiKey, body: &str, deadline: Duration) -> io::Result<String>;
}

/// The explicit opt-in, stored beside the other settings; off when absent.
#[derive(Debug)]
pub struct OptIn {
    store: crate::display_mode_store::Store,
}

impl OptIn {
    /// # Errors
    /// Neither the config directory nor home is available.
    pub fn from_env() -> io::Result<Self> {
        Ok(Self {
            store: crate::display_mode_store::Store::from_env()?.sibling("speech-resolver"),
        })
    }

    #[must_use]
    pub const fn stored_at(store: crate::display_mode_store::Store) -> Self {
        Self { store }
    }

    /// Whether the owner turned the resolver on; any read problem means off.
    #[must_use]
    pub fn enabled(&self) -> bool {
        matches!(self.store.read(), Ok(Some(value)) if value == "on")
    }

    /// # Errors
    /// The setting file could not be replaced.
    pub fn set(&self, enabled: bool) -> io::Result<()> {
        self.store.write(if enabled { "on" } else { "off" })
    }
}

#[derive(Debug)]
pub struct Resolver<T> {
    transport: T,
    key: Option<ApiKey>,
}

impl<T: Transport> Resolver<T> {
    pub const fn new(transport: T, key: Option<ApiKey>) -> Self {
        Self { transport, key }
    }

    /// The model's answer, or `None` when disabled, keyless, failed or late.
    ///
    /// Nothing is sent unless `opted_in` is true, whatever the key.
    pub fn ask(&self, opted_in: bool, transcript: &str, commands: &[String]) -> Option<Answer> {
        if !opted_in || commands.is_empty() || transcript.trim().is_empty() {
            return None;
        }
        let key = self.key.as_ref()?;
        let body = request_body(transcript, commands).to_string();
        let reply = match self.transport.post(key, &body, DEADLINE) {
            Ok(reply) => reply,
            Err(error) => {
                tracing::info!(%error, "speech resolver unavailable");
                return None;
            }
        };
        parse_answer(&reply)
    }
}

/// The request: the transcript as state, one Choice over the command names.
#[must_use]
pub fn request_body(transcript: &str, commands: &[String]) -> serde_json::Value {
    let mut criteria: serde_json::Map<String, serde_json::Value> = commands
        .iter()
        .map(|name| (name.clone(), serde_json::Value::String(name.clone())))
        .collect();
    let _previous = criteria.insert(NO_COMMAND.into(), NO_COMMAND.into());
    serde_json::json!({
        "model": MODEL,
        "state": {"transcript": transcript},
        "questions": {
            QUESTION: {
                "type": "choice",
                "instructions": "Which command did the speaker mean by this transcript?",
                "criteria": criteria,
            }
        }
    })
}

/// Reads a System One reply into an answer; `None` when it is not one.
#[must_use]
pub fn parse_answer(reply: &str) -> Option<Answer> {
    let value: serde_json::Value = serde_json::from_str(reply).ok()?;
    let answer = value.get("answers")?.get(QUESTION)?;
    let command = answer.get("choice")?.as_str()?.to_owned();
    let confidence = thousandths(answer.get("confidence")?.as_f64()?);
    let ranked = answer
        .get("probabilities")?
        .as_object()?
        .iter()
        .filter_map(|(name, probability)| {
            Some(Ranked {
                command: name.clone(),
                probability: thousandths(probability.as_f64()?),
            })
        })
        .collect();
    Some(Answer {
        command,
        confidence,
        ranked,
    })
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    reason = "a probability in 0..=1 becomes thousandths after clamping"
)]
fn thousandths(fraction: f64) -> u16 {
    if fraction.is_nan() {
        return 0;
    }
    (fraction.clamp(0.0, 1.0) * 1000.0).round() as u16
}

/// Posts through `curl`, so the key never appears in the argument list and no
/// network library is linked.
#[derive(Debug)]
pub struct Curl {
    program: PathBuf,
}

impl Default for Curl {
    fn default() -> Self {
        Self::at("curl".into())
    }
}

impl Curl {
    /// The package that provides the program, for the missing-program warning.
    pub const PACKAGE: &'static str = "curl";

    #[must_use]
    pub const fn at(program: PathBuf) -> Self {
        Self { program }
    }

    /// Whether the program can be started; warns once with the package name when it cannot.
    #[must_use]
    pub fn available(&self) -> bool {
        match std::process::Command::new(&self.program)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(
                    %error,
                    package = Self::PACKAGE,
                    "the speech resolver needs curl; install the package to use it"
                );
                false
            }
        }
    }
}

impl Transport for Curl {
    fn post(&self, key: &ApiKey, body: &str, deadline: Duration) -> io::Result<String> {
        let mut child = crate::child_process::command(&self.program)
            .args([
                "-q",
                "--silent",
                "--fail",
                "--max-time",
                &format!("{:.3}", deadline.as_secs_f64()),
                "--config",
                "-",
                ENDPOINT,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let configuration = format!(
            "request = \"POST\"\nheader = \"Content-Type: application/json\"\nheader = \"Authorization: Bearer {}\"\ndata = \"{}\"\n",
            key.0,
            escape(body),
        );
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("curl stdin missing"))?;
        stdin.write_all(configuration.as_bytes())?;
        drop(stdin);
        let started = Instant::now();
        let limit = deadline.saturating_add(Duration::from_millis(100));
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() > limit {
                child.kill()?;
                let _status = child.wait()?;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "speech resolver timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        if !status.success() {
            return Err(io::Error::other("speech resolver request failed"));
        }
        let mut reply = String::new();
        let _bytes = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("curl stdout missing"))?
            .take(1_048_576)
            .read_to_string(&mut reply)?;
        Ok(reply)
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The per-turn log: one JSON line per turn, appended.
#[derive(Debug)]
pub struct TurnLog {
    path: PathBuf,
}

impl TurnLog {
    #[must_use]
    pub const fn stored_at(path: PathBuf) -> Self {
        Self { path }
    }

    /// # Errors
    /// Neither the config directory nor home is available.
    pub fn from_env() -> io::Result<Self> {
        Ok(Self::stored_at(
            crate::display_mode_store::config_root()?.join("tehuti/speech-turns.jsonl"),
        ))
    }

    /// Appends one turn; `cancelled` is whether an undo followed within the window.
    ///
    /// # Errors
    /// The log could not be opened or written.
    pub fn append(&self, turn: &Turn, cancelled: bool) -> io::Result<()> {
        use tehuti_core::speech::resolver::Outcome;
        let (kind, commands): (&str, Vec<&String>) = match &turn.outcome {
            Outcome::Ran(command) => ("ran", vec![command]),
            Outcome::Offered(commands) => ("offered", commands.iter().collect()),
            Outcome::Dictated => ("dictated", Vec::new()),
        };
        let line = serde_json::json!({
            "heard": turn.heard,
            "chosen": turn.chosen.as_ref().map(|(command, confidence)| {
                serde_json::json!({"command": command, "confidence": confidence})
            }),
            "outcome": kind,
            "commands": commands,
            "cancelled": cancelled,
        });
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")
    }
}
