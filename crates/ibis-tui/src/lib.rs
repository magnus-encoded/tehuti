#![expect(
    clippy::multiple_crate_versions,
    reason = "Ratatui requires older rustix and unicode-width alongside workspace dependencies; its macros also use syn 3"
)]
//! Terminal input and command execution for the Ibis builder.
pub mod agent;

use core::time::Duration;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ibis_commands::{Button, Environment, Input, ReadyInvocation};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};

/// Text waits for Enter, or a trailing-space transcript waits for quiet.
#[derive(Debug, Default)]
pub struct InputLine {
    text: String,
}

impl InputLine {
    /// The pending transcript or manually typed text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Submit Tehuti's trailing-space transcript after 400 ms without keys.
    pub fn quiet(&mut self, elapsed: Duration) -> Option<Input> {
        if elapsed >= Duration::from_millis(400) && self.text.ends_with(' ') {
            self.submit()
        } else {
            None
        }
    }

    fn submit(&mut self) -> Option<Input> {
        let text = core::mem::take(&mut self.text);
        (!text.trim().is_empty()).then(|| Input::Say(text.trim().to_owned()))
    }

    /// Map function keys and navigation keys, keeping ordinary text intact.
    pub fn key(&mut self, key: KeyEvent) -> Option<Input> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let button = match key.code {
            KeyCode::F(1) => Some(Button::Accept),
            KeyCode::F(2) | KeyCode::Esc => Some(Button::Back),
            KeyCode::F(3) => Some(Button::Delete),
            KeyCode::F(4) => Some(Button::New),
            KeyCode::F(5) => Some(Button::PreviousStage),
            KeyCode::F(6) => Some(Button::NextStage),
            KeyCode::F(7) => Some(Button::LsUp),
            KeyCode::F(8) => Some(Button::LsDown),
            KeyCode::F(9) => Some(Button::LsLeft),
            KeyCode::F(10) => Some(Button::LsRight),
            KeyCode::Up => Some(Button::ListUp),
            KeyCode::Down => Some(Button::Down),
            KeyCode::Left => Some(Button::Left),
            KeyCode::Right => Some(Button::Right),
            KeyCode::Enter if self.text.is_empty() => Some(Button::Accept),
            KeyCode::Enter => return self.submit(),
            KeyCode::Backspace => {
                let _last = self.text.pop();
                return None;
            }
            KeyCode::Char(character) => {
                self.text.push(character);
                return None;
            }
            KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Delete
            | KeyCode::Insert
            | KeyCode::F(_)
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => None,
        };
        button.map(Input::Press)
    }
}

/// Programs on `PATH`, plus the names and subfolders of `cwd`.
#[must_use]
pub fn environment(cwd: &Path) -> Environment {
    let mut environment = Environment::default();
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            if let Ok(entries) = std::fs::read_dir(path) {
                environment.programs.extend(
                    entries
                        .flatten()
                        .map(|entry| entry.file_name().to_string_lossy().into_owned()),
                );
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(cwd) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                environment.folders.push(name.clone());
            }
            environment.names.push(name);
        }
    }
    environment.programs.sort();
    environment.programs.dedup();
    environment.names.sort();
    environment.folders.sort();
    environment
}

/// Capture combined output without pipe deadlocks; GNU timeout kills the process group.
///
/// # Errors
/// Returns errors starting the command or accessing the temporary capture file.
pub fn run(invocation: &ReadyInvocation, cwd: &Path) -> io::Result<(i32, String)> {
    let mut capture = tempfile::tempfile()?;
    let status = Command::new("timeout")
        .args(["--signal=KILL", "10s", "sh", "-c", &invocation.shell_text()])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(capture.try_clone()?)
        .stderr(capture.try_clone()?)
        .status()?;
    let length = capture.metadata()?.len();
    let _position = capture.seek(SeekFrom::Start(length.saturating_sub(256 * 1024)))?;
    let mut bytes = Vec::new();
    let _count = capture.read_to_end(&mut bytes)?;
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    let exit = status.code().unwrap_or(137);
    if exit == 137 || exit == 124 {
        output.push_str("\n(command killed or timed out after 10 s)");
    }
    Ok((exit, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibis_commands::{Effect, State, specs, step};
    use tehuti_protocol::{Action, ButtonName, KeyName};

    fn key(line: &mut InputLine, code: KeyCode) -> Option<Input> {
        line.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn voice_burst_and_accept_build_a_ready_command_then_run() -> Result<(), Box<dyn core::error::Error>> {
        let mut state = State::new(".".into(), specs()?);
        let mut line = InputLine::default();
        let new_command = key(&mut line, KeyCode::F(4)).ok_or("missing new command")?;
        (state, _) = step(state, new_command);
        for character in "echo hello ".chars() {
            assert!(key(&mut line, KeyCode::Char(character)).is_none());
        }
        let input = line
            .quiet(Duration::from_millis(500))
            .ok_or("missing transcript")?;
        (state, _) = step(state, input);
        let accept = key(&mut line, KeyCode::F(1)).ok_or("missing accept")?;
        (state, _) = step(state, accept);
        assert!(state.screen().ready);
        let run_command = key(&mut line, KeyCode::Enter).ok_or("missing enter")?;
        let (_, effects) = step(state, run_command);
        assert!(matches!(effects.as_slice(), [Effect::Run(_)]));
        Ok(())
    }

    #[test]
    fn back_closes_choices_without_accepting_the_transcript() -> Result<(), Box<dyn core::error::Error>> {
        let mut state = State::new(".".into(), specs()?);
        let mut line = InputLine::default();
        for code in core::iter::once(KeyCode::F(4))
            .chain("echo hello".chars().map(KeyCode::Char))
            .chain(core::iter::once(KeyCode::Enter))
        {
            if let Some(input) = key(&mut line, code) {
                let effects;
                (state, effects) = step(state, input);
                assert_eq!(effects, []);
            }
        }
        assert_ne!(state.screen().list, []);
        let back = key(&mut line, KeyCode::F(2)).ok_or("missing back")?;
        let (state, effects) = step(state, back);
        assert_eq!(effects, []);
        assert_eq!(state.screen().list, []);
        assert!(!state.screen().ready);
        Ok(())
    }

    #[test]
    fn ready_command_captures_output_and_exit_status() -> Result<(), Box<dyn core::error::Error>> {
        let mut state = State::new(".".into(), specs()?);
        for input in [
            Input::Press(Button::New),
            Input::Say("echo hello".into()),
            Input::Press(Button::Accept),
        ] {
            (state, _) = step(state, input);
        }
        let invocation = state.draft().ready().ok_or("command is not ready")?;
        let (exit, output) = run(&invocation, Path::new("."))?;
        assert_eq!(exit, 0);
        assert_eq!(output.trim(), "hello");
        Ok(())
    }

    #[test]
    fn manual_text_waits_for_enter_and_backspace_edits() {
        let mut line = InputLine::default();
        for character in "pwdx".chars() {
            let _input = key(&mut line, KeyCode::Char(character));
        }
        assert!(line.quiet(Duration::from_secs(1)).is_none());
        let _input = key(&mut line, KeyCode::Backspace);
        assert!(matches!(key(&mut line, KeyCode::Enter), Some(Input::Say(text)) if text == "pwd"));
    }

    #[test]
    fn shipped_context_maps_sticks_and_buttons() -> Result<(), String> {
        let context =
            tehuti_protocol::parse_context(include_str!("../../../dist/contexts/ibis-tui/context.toml"))?;
        assert_eq!(context.app_id(), "ibis-tui");
        assert_eq!(context.key_for(ButtonName::South), Some(KeyName::F1));
        assert_eq!(context.key_for(ButtonName::North), Some(KeyName::F3));
        assert_eq!(context.key_for(ButtonName::West), Some(KeyName::F4));
        assert_eq!(
            context.stick_actions((0, -1000), (0, 1000)),
            [
                Action::Key {
                    key: KeyName::F7,
                    mods: vec![]
                },
                Action::Key {
                    key: KeyName::PageDown,
                    mods: vec![]
                },
            ]
        );
        assert_eq!(context.stick_actions((0, 0), (0, 0)), []);
        Ok(())
    }
}
