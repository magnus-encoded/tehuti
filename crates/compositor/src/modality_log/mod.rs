//! The modality log: which input kind carried each action, kept on this machine to show where the keyboard is still needed.
//!
//! A record holds an input kind, a key class, the focused app id and a time. It never holds typed text, key codes
//! of text keys or transcripts: the record types have no field that could carry them.

use core::time::Duration;
use std::{
    io,
    io::Write as _,
    path::{Path, PathBuf},
};

pub mod report;

/// The input kind that carried one action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Keyboard,
    Pad,
    Voice,
    Pointer,
    /// A pad press while the on-screen keyboard is open.
    Osk,
}

impl Modality {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Pad => "pad",
            Self::Voice => "voice",
            Self::Pointer => "pointer",
            Self::Osk => "osk",
        }
    }
}

/// What a hardware key press did, without which key it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyClass {
    /// Letters, digits, punctuation and space with no command modifier held.
    TextEntry,
    /// Any key while Ctrl, Alt or Meta is held.
    Shortcut,
    /// Arrows, paging, Home, End, Tab and Escape.
    Navigation,
    /// Backspace, Delete, Enter and Insert.
    Editing,
    /// F1 to F12.
    Function,
}

impl KeyClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextEntry => "text entry",
            Self::Shortcut => "shortcut",
            Self::Navigation => "navigation",
            Self::Editing => "editing",
            Self::Function => "function",
        }
    }
}

/// A view that opened during one action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Launcher,
    SystemMenu,
    Keyboard,
    ChoiceList,
}

impl Surface {
    pub const ALL: [Self; 4] = [Self::Launcher, Self::SystemMenu, Self::Keyboard, Self::ChoiceList];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::SystemMenu => "system menu",
            Self::Keyboard => "on-screen keyboard",
            Self::ChoiceList => "choice list",
        }
    }
}

/// What one transcript became, judged before it is routed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechOutcome {
    /// It names a command of the focused context.
    Matched,
    /// It names no command and the on-screen keyboard is closed.
    Unmatched,
    /// The on-screen keyboard or a password field takes it as text.
    Dictated,
}

/// One entry of the modality log.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "name", rename_all = "snake_case")]
pub enum Record {
    SessionStart,
    SessionEnd,
    Input {
        modality: Modality,
        app_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_class: Option<KeyClass>,
    },
    Speech {
        outcome: SpeechOutcome,
        app_id: Option<String>,
    },
    /// A keyboard action soon after an unmatched transcript, with no matched one between.
    Fallback {
        app_id: Option<String>,
        key_class: KeyClass,
        after_ms: u64,
    },
    SurfaceOpened {
        surface: Surface,
        modality: Modality,
        app_id: Option<String>,
    },
}

/// One line of the log file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    pub time_unix_ms: u64,
    #[serde(flatten)]
    pub record: Record,
}

/// When an action happened: the session clock for intervals, wall time for the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub elapsed: Duration,
    pub unix_ms: u64,
}

const KEY_ESC: u32 = 1;
const KEY_BACKSPACE: u32 = 14;
const KEY_TAB: u32 = 15;
const KEY_ENTER: u32 = 28;
const KEY_LEFTCTRL: u32 = 29;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_RIGHTSHIFT: u32 = 54;
const KEY_LEFTALT: u32 = 56;
const KEY_CAPSLOCK: u32 = 58;
const KEY_F1: u32 = 59;
const KEY_F10: u32 = 68;
const KEY_F11: u32 = 87;
const KEY_F12: u32 = 88;
const KEY_KPENTER: u32 = 96;
const KEY_RIGHTCTRL: u32 = 97;
const KEY_RIGHTALT: u32 = 100;
const KEY_HOME: u32 = 102;
const KEY_UP: u32 = 103;
const KEY_PAGEUP: u32 = 104;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;
const KEY_END: u32 = 107;
const KEY_DOWN: u32 = 108;
const KEY_PAGEDOWN: u32 = 109;
const KEY_INSERT: u32 = 110;
const KEY_DELETE: u32 = 111;
const KEY_LEFTMETA: u32 = 125;
const KEY_RIGHTMETA: u32 = 126;

/// Ctrl, Alt or Meta: a key pressed while one is held is a shortcut.
#[must_use]
pub const fn is_command_modifier(code: u32) -> bool {
    matches!(
        code,
        KEY_LEFTCTRL | KEY_RIGHTCTRL | KEY_LEFTALT | KEY_RIGHTALT | KEY_LEFTMETA | KEY_RIGHTMETA
    )
}

/// The class of one pressed key, from its evdev code; `None` for a modifier alone, which is no action.
#[must_use]
pub const fn classify(code: u32, command_modifier_held: bool) -> Option<KeyClass> {
    if is_command_modifier(code) || matches!(code, KEY_LEFTSHIFT | KEY_RIGHTSHIFT | KEY_CAPSLOCK) {
        return None;
    }
    if command_modifier_held {
        return Some(KeyClass::Shortcut);
    }
    Some(match code {
        KEY_ESC | KEY_TAB | KEY_HOME | KEY_UP | KEY_PAGEUP | KEY_LEFT | KEY_RIGHT | KEY_END | KEY_DOWN
        | KEY_PAGEDOWN => KeyClass::Navigation,
        KEY_BACKSPACE | KEY_ENTER | KEY_KPENTER | KEY_INSERT | KEY_DELETE => KeyClass::Editing,
        KEY_F1..=KEY_F10 | KEY_F11 | KEY_F12 => KeyClass::Function,
        _ => KeyClass::TextEntry,
    })
}

/// Watches for a keyboard action that follows an unmatched transcript.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FallbackWatch {
    unmatched_at: Option<Duration>,
}

impl FallbackWatch {
    /// How long after an unmatched transcript a keyboard action still counts as its fallback.
    pub const WINDOW: Duration = Duration::from_secs(10);

    /// A new transcript replaces any earlier one: only an unmatched one is watched.
    pub fn speech(&mut self, outcome: SpeechOutcome, when: Duration) {
        self.unmatched_at = matches!(outcome, SpeechOutcome::Unmatched).then_some(when);
    }

    /// The time since the watched transcript when this keyboard action is its fallback; each transcript has at most one.
    pub fn keyboard(&mut self, when: Duration) -> Option<Duration> {
        let since = when.checked_sub(self.unmatched_at.take()?)?;
        (since <= Self::WINDOW).then_some(since)
    }
}

/// Where log lines go.
pub trait Sink: core::fmt::Debug {
    /// Appends one line, without its newline.
    ///
    /// # Errors
    /// The line could not be stored.
    fn append(&mut self, line: &str) -> io::Result<()>;
}

impl Sink for Vec<String> {
    fn append(&mut self, line: &str) -> io::Result<()> {
        self.push(line.to_owned());
        Ok(())
    }
}

/// Turns input notices into log lines: tracks held modifiers and fallbacks, and drops what is no action.
#[derive(Debug)]
pub struct Recorder<S: Sink> {
    sink: S,
    held_command_modifiers: Vec<u32>,
    fallback: FallbackWatch,
    failing: bool,
}

impl<S: Sink> Recorder<S> {
    /// Opens a session span.
    #[must_use]
    pub fn start(sink: S, when: Stamp) -> Self {
        let mut recorder = Self {
            sink,
            held_command_modifiers: Vec::new(),
            fallback: FallbackWatch::default(),
            failing: false,
        };
        recorder.write(when, Record::SessionStart);
        recorder
    }

    /// Closes the session span and hands back the sink.
    #[must_use]
    pub fn end(mut self, when: Stamp) -> S {
        self.write(when, Record::SessionEnd);
        self.sink
    }

    /// One hardware key change, by evdev code.
    pub fn key(&mut self, code: u32, pressed: bool, app_id: Option<&str>, when: Stamp) {
        if is_command_modifier(code) {
            self.held_command_modifiers.retain(|held| *held != code);
            if pressed {
                self.held_command_modifiers.push(code);
            }
            return;
        }
        if !pressed {
            return;
        }
        let Some(key_class) = classify(code, !self.held_command_modifiers.is_empty()) else {
            return;
        };
        self.write(
            when,
            Record::Input {
                modality: Modality::Keyboard,
                app_id: app_id.map(str::to_owned),
                key_class: Some(key_class),
            },
        );
        if let Some(after) = self.fallback.keyboard(when.elapsed) {
            self.write(
                when,
                Record::Fallback {
                    app_id: app_id.map(str::to_owned),
                    key_class,
                    after_ms: u64::try_from(after.as_millis()).unwrap_or(u64::MAX),
                },
            );
        }
    }

    /// Every hardware key went up, such as when the seat is lost.
    pub fn release_keys(&mut self) {
        self.held_command_modifiers.clear();
    }

    /// A pad, pointer or on-screen keyboard press.
    pub fn press(&mut self, modality: Modality, app_id: Option<&str>, when: Stamp) {
        self.write(
            when,
            Record::Input {
                modality,
                app_id: app_id.map(str::to_owned),
                key_class: None,
            },
        );
    }

    /// One transcript arrived; its text is not kept.
    pub fn speech(&mut self, outcome: SpeechOutcome, app_id: Option<&str>, when: Stamp) {
        self.fallback.speech(outcome, when.elapsed);
        self.write(
            when,
            Record::Speech {
                outcome,
                app_id: app_id.map(str::to_owned),
            },
        );
    }

    pub fn surface_opened(
        &mut self,
        surface: Surface,
        modality: Modality,
        app_id: Option<&str>,
        when: Stamp,
    ) {
        self.write(
            when,
            Record::SurfaceOpened {
                surface,
                modality,
                app_id: app_id.map(str::to_owned),
            },
        );
    }

    fn write(&mut self, when: Stamp, record: Record) {
        let line = Line {
            time_unix_ms: when.unix_ms,
            record,
        };
        let result = serde_json::to_string(&line)
            .map_err(io::Error::other)
            .and_then(|text| self.sink.append(&text));
        match result {
            Ok(()) => {
                if self.failing {
                    self.failing = false;
                    tracing::info!("modality log writes again");
                }
            }
            Err(error) => {
                if !self.failing {
                    self.failing = true;
                    tracing::warn!(%error, "modality log not written");
                }
            }
        }
    }
}

/// An append-only JSON Lines file that moves to `<name>.1` when the next line would pass the cap.
///
/// The two files together stay under twice the cap.
#[derive(Debug)]
pub struct FileSink {
    path: PathBuf,
    cap_bytes: u64,
    file: Option<std::fs::File>,
    written: u64,
}

impl FileSink {
    pub const DEFAULT_CAP_BYTES: u64 = 8_388_608;

    /// # Errors
    /// The directory or file could not be created or read.
    pub fn open(path: PathBuf, cap_bytes: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = Self::append_to(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            cap_bytes,
            file: Some(file),
            written,
        })
    }

    /// The file the log moves to when it reaches the cap.
    #[must_use]
    pub fn rotated(path: &Path) -> PathBuf {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".1");
        path.with_file_name(name)
    }

    fn append_to(path: &Path) -> io::Result<std::fs::File> {
        std::fs::OpenOptions::new().create(true).append(true).open(path)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        std::fs::rename(&self.path, Self::rotated(&self.path))?;
        self.file = Some(Self::append_to(&self.path)?);
        self.written = 0;
        Ok(())
    }
}

impl Sink for FileSink {
    fn append(&mut self, line: &str) -> io::Result<()> {
        let mut bytes = String::with_capacity(line.len().saturating_add(1));
        bytes.push_str(line);
        bytes.push('\n');
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self.written > 0 && self.written.saturating_add(size) > self.cap_bytes {
            self.rotate()?;
        }
        if self.file.is_none() {
            self.file = Some(Self::append_to(&self.path)?);
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("modality log file closed"))?;
        file.write_all(bytes.as_bytes())?;
        self.written = self.written.saturating_add(size);
        Ok(())
    }
}

/// The recorder a session runs with.
pub type SessionRecorder = Recorder<FileSink>;

/// `$XDG_STATE_HOME/tehuti/modality.jsonl`, otherwise under `$HOME/.local/state`.
///
/// # Errors
/// Neither the state directory nor home is available.
pub fn default_path() -> io::Result<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|root| root.join("tehuti/modality.jsonl"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "state directory unavailable"))
}

/// The switch file: `off` on its first line turns the log off; absent means on; unreadable means off.
#[must_use]
pub fn enabled(switch: &crate::display_mode_store::Store) -> bool {
    match switch.read() {
        Ok(None) => true,
        Ok(Some(value)) => value != "off",
        Err(_) => false,
    }
}

/// The switch file beside the other settings: `~/.config/tehuti/modality-log`.
///
/// # Errors
/// Neither the config directory nor home is available.
pub fn switch_from_env() -> io::Result<crate::display_mode_store::Store> {
    Ok(crate::display_mode_store::Store::new(
        crate::display_mode_store::config_root()?.join("tehuti/modality-log"),
    ))
}

/// Wall time now, in milliseconds since the Unix epoch; zero before it.
#[must_use]
pub fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
