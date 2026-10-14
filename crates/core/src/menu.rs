//! The system menu: a core-owned overlay that takes all input while open.
//!
//! It opens from the Guide hold and works even when a plugin or application
//! is stuck. The menu never performs I/O; choosing an item yields a [`Command`]
//! or an action for the compositor to carry out.

use tehuti_protocol::{Action, ButtonName, ButtonState, Event};

/// A system operation the compositor carries out. Plugins can never request one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Screenshot,
    Lock,
    /// Stop Tehuti with exit status 21 so the session ends.
    LogOut,
    /// Stop Tehuti with exit status 20 so the session starts it again.
    Restart,
    Reboot,
    PowerOff,
}

/// One row of the menu, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    CloseApp,
    Screenshot,
    Lock,
    LogOut,
    RestartTehuti,
    Reboot,
    PowerOff,
    Cancel,
}

pub const ITEMS: [Item; 8] = [
    Item::CloseApp,
    Item::Screenshot,
    Item::Lock,
    Item::LogOut,
    Item::RestartTehuti,
    Item::Reboot,
    Item::PowerOff,
    Item::Cancel,
];

impl Item {
    /// Upper case text the notice font can draw.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CloseApp => "CLOSE APP",
            Self::Screenshot => "SCREENSHOT",
            Self::Lock => "LOCK",
            Self::LogOut => "LOG OUT",
            Self::RestartTehuti => "RESTART TEHUTI",
            Self::Reboot => "REBOOT",
            Self::PowerOff => "POWER OFF",
            Self::Cancel => "CANCEL",
        }
    }
}

/// An open menu and its highlighted row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    selected: usize,
}

impl Menu {
    #[must_use]
    pub fn selected(&self) -> Item {
        ITEMS.get(self.selected).copied().unwrap_or(Item::Cancel)
    }

    /// The overlay text: a title line, then one line per item.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::from("SYSTEM MENU");
        for (index, item) in ITEMS.iter().enumerate() {
            text.push('\n');
            text.push_str(if index == self.selected { "> " } else { "  " });
            text.push_str(item.label());
        }
        text
    }
}

/// What one input did to the menu.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub actions: Vec<Action>,
    pub commands: Vec<Command>,
}

/// Applies one input to an open menu; every input is consumed. `None` in `menu` after the call means closed.
pub fn handle(menu: &mut Option<Menu>, event: &Event) -> Outcome {
    let mut outcome = Outcome::default();
    let Event::Button {
        name,
        state: ButtonState::Pressed,
    } = event
    else {
        return outcome;
    };
    let Some(open) = menu.as_mut() else {
        return outcome;
    };
    let count = ITEMS.len();
    match name {
        ButtonName::DpadUp => {
            open.selected = open
                .selected
                .checked_sub(1)
                .unwrap_or_else(|| count.saturating_sub(1));
        }
        ButtonName::DpadDown => {
            open.selected = open.selected.saturating_add(1).checked_rem(count).unwrap_or(0);
        }
        ButtonName::East | ButtonName::Guide => *menu = None,
        ButtonName::South => {
            let item = open.selected();
            *menu = None;
            match item {
                Item::CloseApp => outcome.actions.push(Action::Close),
                Item::Screenshot => outcome.commands.push(Command::Screenshot),
                Item::Lock => outcome.commands.push(Command::Lock),
                Item::LogOut => outcome.commands.push(Command::LogOut),
                Item::RestartTehuti => outcome.commands.push(Command::Restart),
                Item::Reboot => outcome.commands.push(Command::Reboot),
                Item::PowerOff => outcome.commands.push(Command::PowerOff),
                Item::Cancel => {}
            }
        }
        ButtonName::North
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
    outcome
}
