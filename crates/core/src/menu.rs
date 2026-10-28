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
    Custom(usize),
}

impl Command {
    #[must_use]
    pub const fn key(self) -> &'static str {
        self.presentation().0
    }

    const fn label(self) -> &'static str {
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
            Self::Custom(_) => ("entry", ""),
        }
    }
}

const ITEMS: [Command; 7] = [
    Command::LogOut,
    Command::Restart,
    Command::Plasma,
    Command::Reboot,
    Command::ShutDown,
    Command::TaskMonitor,
    Command::Settings,
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    pad_family: PadFamily,
    entries: Vec<String>,
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
    pub fn with_entries(mut self, entries: Vec<String>) -> Self {
        self.entries = entries;
        self
    }

    fn command(&self) -> Option<Command> {
        ITEMS.get(self.selected).copied().or_else(|| {
            self.entries
                .get(self.selected.saturating_sub(ITEMS.len()))
                .map(|_| Command::Custom(self.selected.saturating_sub(ITEMS.len())))
        })
    }

    fn label(&self) -> &str {
        ITEMS.get(self.selected).map_or_else(
            || {
                self.entries
                    .get(self.selected.saturating_sub(ITEMS.len()))
                    .map_or("CANCEL", String::as_str)
            },
            |item| item.label(),
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
                Command::TaskMonitor | Command::Settings | Command::Custom(_)
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
        let last = ITEMS.len().saturating_add(self.entries.len()).saturating_sub(1);
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
        for (index, label) in ITEMS
            .iter()
            .map(|item| item.label())
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
