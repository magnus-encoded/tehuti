//! Controller navigation and confirmation for the system menu.

use crate::pad::{PadFamily, Role};
use tehuti_protocol::{ButtonName, ButtonState, Event};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    LogOut,
    Restart,
    Plasma,
    Reboot,
    ShutDown,
    TaskMonitor,
    Settings,
    Screenshot,
    StartRecording,
    StopRecording,
    ToggleRecording,
    Custom(usize),
}

impl Command {
    #[must_use]
    pub const fn key(self) -> &'static str {
        self.presentation().0
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        self.presentation().1
    }

    const fn presentation(self) -> (&'static str, &'static str) {
        match self {
            Self::LogOut => ("log_out", "LOG OUT"),
            Self::Restart => ("restart", "RESTART TEHUTI"),
            Self::Plasma => ("plasma", "SWITCH TO PLASMA"),
            Self::Reboot => ("reboot", "REBOOT"),
            Self::ShutDown => ("shut_down", "SHUT DOWN"),
            Self::TaskMonitor => ("task_monitor", "TASK MONITOR"),
            Self::Settings => ("settings", "SETTINGS"),
            Self::Screenshot => ("screenshot", "SCREENSHOT"),
            Self::StartRecording => ("start_recording", "START RECORDING"),
            Self::ToggleRecording => ("toggle_recording", "START RECORDING"),
            Self::StopRecording => ("stop_recording", "STOP RECORDING"),
            Self::Custom(_) => ("entry", ""),
        }
    }
}

pub const ITEMS: [Command; 9] = [
    Command::LogOut,
    Command::Restart,
    Command::Plasma,
    Command::Reboot,
    Command::ShutDown,
    Command::TaskMonitor,
    Command::Settings,
    Command::Screenshot,
    Command::ToggleRecording,
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    recording: bool,
    pad_family: PadFamily,
    entries: Vec<String>,
    hidden: Vec<Command>,
    selected: usize,
    confirming: bool,
    confirm_held: bool,
}

impl Menu {
    #[must_use]
    pub fn new(pad_family: PadFamily) -> Self {
        Self {
            pad_family,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn for_command(pad_family: PadFamily, command: Command) -> Self {
        Self::new(pad_family).selecting(command)
    }

    /// Selects `command` when it is shown, else the first row.
    #[must_use]
    pub fn selecting(mut self, command: Command) -> Self {
        let selected = self.items().position(|item| item == command).unwrap_or_default();
        self.selected = selected;
        self
    }

    /// Leaves out built-in commands that have nothing to run.
    #[must_use]
    pub fn with_hidden(mut self, hidden: Vec<Command>) -> Self {
        self.hidden = hidden;
        self.selected = 0;
        self
    }

    fn items(&self) -> impl Iterator<Item = Command> + '_ {
        ITEMS.into_iter().filter(|item| !self.hidden.contains(item))
    }

    fn item_count(&self) -> usize {
        self.items().count()
    }

    #[must_use]
    pub fn with_entries(mut self, entries: Vec<String>) -> Self {
        self.entries = entries;
        self
    }

    #[must_use]
    pub const fn with_recording(mut self, recording: bool) -> Self {
        self.recording = recording;
        self
    }

    fn item_label(&self, item: Command) -> &'static str {
        if item == Command::ToggleRecording && self.recording {
            "STOP RECORDING"
        } else {
            item.label()
        }
    }

    fn command(&self) -> Option<Command> {
        let count = self.item_count();
        self.items().nth(self.selected).or_else(|| {
            self.entries
                .get(self.selected.saturating_sub(count))
                .map(|_| Command::Custom(self.selected.saturating_sub(count)))
        })
    }

    fn label(&self) -> &str {
        self.items().nth(self.selected).map_or_else(
            || {
                self.entries
                    .get(self.selected.saturating_sub(self.item_count()))
                    .map_or("CANCEL", String::as_str)
            },
            |item| self.item_label(item),
        )
    }

    pub const fn set_pad_family(&mut self, family: PadFamily) {
        self.pad_family = family;
    }

    fn confirm(&mut self) -> Option<Command> {
        let command = self.command()?;
        if self.confirming
            || matches!(
                command,
                Command::TaskMonitor
                    | Command::Settings
                    | Command::Screenshot
                    | Command::StartRecording
                    | Command::StopRecording
                    | Command::ToggleRecording
                    | Command::Custom(_)
            )
        {
            return Some(command);
        }
        self.confirming = true;
        None
    }

    fn fresh_press(&mut self, name: ButtonName, state: ButtonState) -> bool {
        if name == self.pad_family.button(Role::Confirm) {
            let was_held = core::mem::replace(&mut self.confirm_held, state == ButtonState::Pressed);
            return !was_held && state == ButtonState::Pressed;
        }
        state == ButtonState::Pressed
    }

    fn move_selection(&mut self, upward: bool) {
        self.confirming = false;
        let last = self
            .item_count()
            .saturating_add(self.entries.len())
            .saturating_sub(1);
        self.selected = if upward {
            self.selected.checked_sub(1).unwrap_or(last)
        } else if self.selected == last {
            0
        } else {
            self.selected.saturating_add(1)
        };
    }

    #[must_use]
    pub fn text(&self) -> String {
        let confirm = self.pad_family.label(self.pad_family.button(Role::Confirm));
        let back = self.pad_family.label(self.pad_family.button(Role::Back));
        if self.confirming {
            let label = self.label();
            return format!("CONFIRM {label}?\n{confirm} CONFIRM / {back} CANCEL");
        }
        let mut text = String::from("SYSTEM MENU");
        for (index, label) in self
            .items()
            .map(|item| self.item_label(item))
            .chain(self.entries.iter().map(String::as_str))
            .enumerate()
        {
            text.push_str(if index == self.selected { "\n> " } else { "\n  " });
            text.push_str(label);
        }
        text.push('\n');
        text.push_str(back);
        text.push_str(" OR GUIDE CLOSE");
        text
    }
}

/// Consumes menu input; commands require a fresh confirm press after confirmation appears.
pub fn handle(menu: &mut Option<Menu>, event: &Event) -> Option<Command> {
    let open = menu.as_mut()?;
    let Event::Button { name, state } = event else {
        return None;
    };
    if !open.fresh_press(*name, *state) {
        return None;
    }
    match name {
        name if *name == open.pad_family.button(Role::Back) || *name == ButtonName::Guide => *menu = None,
        ButtonName::DpadUp | ButtonName::DpadDown => {
            open.move_selection(*name == ButtonName::DpadUp);
        }
        name if *name == open.pad_family.button(Role::Confirm) => {
            let command = open.confirm();
            if command.is_some() {
                *menu = None;
            }
            return command;
        }
        ButtonName::South
        | ButtonName::East
        | ButtonName::Guide
        | ButtonName::North
        | ButtonName::West
        | ButtonName::LeftBumper
        | ButtonName::RightBumper
        | ButtonName::LeftTrigger
        | ButtonName::RightTrigger
        | ButtonName::Select
        | ButtonName::Record
        | ButtonName::Start
        | ButtonName::LeftStick
        | ButtonName::RightStick
        | ButtonName::DpadLeft
        | ButtonName::DpadRight => {}
    }
    None
}
