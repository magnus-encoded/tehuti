//! Controller navigation and confirmation for the system menu.

use tehuti_protocol::{ButtonName, ButtonState, Event};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    LogOut,
    Plasma,
    Reboot,
    ShutDown,
    TaskMonitor,
}

impl Command {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::LogOut => "log_out",
            Self::Plasma => "plasma",
            Self::Reboot => "reboot",
            Self::ShutDown => "shut_down",
            Self::TaskMonitor => "task_monitor",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::LogOut => "LOG OUT",
            Self::Plasma => "SWITCH TO PLASMA",
            Self::Reboot => "REBOOT",
            Self::ShutDown => "SHUT DOWN",
            Self::TaskMonitor => "TASK MONITOR",
        }
    }
}

const ITEMS: [Command; 5] = [
    Command::LogOut,
    Command::Plasma,
    Command::Reboot,
    Command::ShutDown,
    Command::TaskMonitor,
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    selected: usize,
    confirming: bool,
    south_held: bool,
}

impl Menu {
    #[must_use]
    pub fn text(&self) -> String {
        if self.confirming {
            let label = ITEMS.get(self.selected).map_or("CANCEL", |item| item.label());
            return format!("CONFIRM {label}?\nA CONFIRM / B CANCEL");
        }
        let mut text = String::from("SYSTEM MENU");
        for (index, item) in ITEMS.iter().enumerate() {
            text.push_str(if index == self.selected { "\n> " } else { "\n  " });
            text.push_str(item.label());
        }
        text.push_str("\nB OR GUIDE CLOSE");
        text
    }
}

/// Consumes menu input; commands require a fresh A press after confirmation appears.
pub fn handle(menu: &mut Option<Menu>, event: &Event) -> Option<Command> {
    let open = menu.as_mut()?;
    let Event::Button { name, state } = event else {
        return None;
    };
    if *name == ButtonName::South {
        let was_held = core::mem::replace(&mut open.south_held, *state == ButtonState::Pressed);
        if was_held || *state == ButtonState::Released {
            return None;
        }
    } else if *state == ButtonState::Released {
        return None;
    }
    match name {
        ButtonName::East | ButtonName::Guide => *menu = None,
        ButtonName::DpadUp | ButtonName::DpadDown => {
            open.confirming = false;
            open.selected = if *name == ButtonName::DpadUp {
                open.selected.checked_sub(1).unwrap_or(4)
            } else {
                open.selected.saturating_add(1).checked_rem(5).unwrap_or(0)
            };
        }
        ButtonName::South => {
            let command = *ITEMS.get(open.selected)?;
            if open.confirming || command == Command::TaskMonitor {
                *menu = None;
                return Some(command);
            }
            open.confirming = true;
        }
        _ => {}
    }
    None
}
