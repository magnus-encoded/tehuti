//! Release update badge, voice requests, and controller confirmation state.

use crate::{
    Level,
    pad::{PadFamily, Role},
};
use tehuti_protocol::{ButtonName, ButtonState};

/// Voice phrase that asks to apply an available update.
pub const UPDATE_PHRASE: &str = "update";

/// Voice phrase that asks to roll back to the previous release.
pub const PREVIOUS_VERSION_PHRASE: &str = "previous version";

/// The bar chip text shown when a newer release exists; matches [`UPDATE_PHRASE`].
pub const BAR_CHIP_LABEL: &str = "Update";

/// Spoken update unit recognized before context routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phrase {
    Update,
    PreviousVersion,
}

/// Recognizes the reserved update or rollback phrase in `text`.
#[must_use]
pub fn parse_phrase(text: &str) -> Option<Phrase> {
    match tehuti_protocol::normalize_phrase(text).as_str() {
        UPDATE_PHRASE => Some(Phrase::Update),
        PREVIOUS_VERSION_PHRASE => Some(Phrase::PreviousVersion),
        _ => None,
    }
}

/// Installed, fallback and newer release versions reported by the compositor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// Currently installed release tag, when known.
    pub installed: Option<String>,
    /// Previous release tag (`tehuti.prev`) available for rollback, when present.
    pub previous: Option<String>,
    /// Newer release tag available to install, when newer than installed and not skipped.
    pub available: Option<String>,
}

/// An update or rollback awaiting a fresh controller confirmation press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingConfirmation {
    /// Apply the available update to `version`.
    Update { version: String },
    /// Roll back to `version`.
    Rollback {
        version: String,
        /// True when offered automatically after repeated failed starts.
        after_failed_starts: bool,
    },
}

/// Outcome of requesting an update or rollback prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    /// Confirmation prompt is now open.
    Prompted,
    /// No update or previous version is available.
    Unavailable,
    /// An update or rollback script is already running.
    AlreadyRunning,
}

/// Result of a button press while [`PendingConfirmation`] owns the pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationAction {
    /// Button consumed with no script started.
    Consumed,
    /// Prompt cancelled by Back.
    Cancelled,
    /// Fresh Confirm pressed for an update.
    ApplyUpdate,
    /// Fresh Confirm pressed for a rollback.
    ApplyRollback,
}

/// Result of a button event offered to the bar's `Update` chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarAction {
    /// Not handled by the bar chip.
    Ignored,
    /// Consumed by focusing, unfocusing or opening confirmation on the bar chip.
    Consumed,
}

/// Result reported by the compositor after `tehuti-update` finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Update succeeded; `installed` and `previous` reflect the new on-disk tags.
    Updated {
        installed: Option<String>,
        previous: Option<String>,
    },
    /// Rollback succeeded; `installed` and `previous` reflect the swapped tags.
    RolledBack {
        installed: Option<String>,
        previous: Option<String>,
    },
    /// The script exited with an error or could not be started.
    Failed(String),
}

/// Pure core state for the update badge, bar chip focus and confirmation prompts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    info: ReleaseInfo,
    chip_focused: bool,
    confirmation: Option<PendingConfirmation>,
    running: bool,
    confirm_held: bool,
    held_buttons: Vec<ButtonName>,
}

impl UpdateState {
    /// Currently known release tags.
    #[must_use]
    pub const fn info(&self) -> &ReleaseInfo {
        &self.info
    }

    /// Newer release tag to offer on the bar, or `None` while an update is running.
    #[must_use]
    pub fn available(&self) -> Option<&str> {
        (!self.running)
            .then_some(self.info.available.as_deref())
            .flatten()
    }

    /// Previous release tag available for rollback.
    #[must_use]
    pub fn previous(&self) -> Option<&str> {
        self.info.previous.as_deref()
    }

    /// True while D-pad focus rests on the bar's `Update` chip.
    #[must_use]
    pub fn chip_focused(&self) -> bool {
        self.chip_focused && self.available().is_some()
    }

    /// The pending confirmation prompt, if one is open.
    #[must_use]
    pub const fn confirmation(&self) -> Option<&PendingConfirmation> {
        self.confirmation.as_ref()
    }

    /// True while an update or rollback script is executing.
    #[must_use]
    pub const fn running(&self) -> bool {
        self.running
    }

    /// Replaces release metadata from a local read or background check.
    pub fn set_info(&mut self, info: ReleaseInfo) {
        self.info = info;
        if self.info.available.is_none() {
            self.chip_focused = false;
            if matches!(self.confirmation, Some(PendingConfirmation::Update { .. })) {
                self.confirmation = None;
            }
        }
        if self.info.previous.is_none()
            && matches!(self.confirmation, Some(PendingConfirmation::Rollback { .. }))
        {
            self.confirmation = None;
        }
    }

    /// Offers a rollback confirmation after repeated failed starts, when a fallback exists.
    pub fn offer_failed_start_rollback(&mut self) {
        if self.running {
            return;
        }
        if let Some(version) = self.info.previous.clone() {
            self.chip_focused = false;
            self.confirm_held = false;
            self.confirmation = Some(PendingConfirmation::Rollback {
                version,
                after_failed_starts: true,
            });
        }
    }

    /// Opens the update confirmation prompt when a newer release exists.
    pub fn request_update(&mut self, confirm_held: bool) -> RequestOutcome {
        if self.running {
            return RequestOutcome::AlreadyRunning;
        }
        if let Some(version) = self.info.available.clone() {
            self.confirm_held = confirm_held;
            self.confirmation = Some(PendingConfirmation::Update { version });
            RequestOutcome::Prompted
        } else {
            RequestOutcome::Unavailable
        }
    }

    /// Opens the rollback confirmation prompt when a previous version exists.
    pub fn request_rollback(&mut self, confirm_held: bool) -> RequestOutcome {
        if self.running {
            return RequestOutcome::AlreadyRunning;
        }
        if let Some(version) = self.info.previous.clone() {
            self.confirm_held = confirm_held;
            self.confirmation = Some(PendingConfirmation::Rollback {
                version,
                after_failed_starts: false,
            });
            RequestOutcome::Prompted
        } else {
            RequestOutcome::Unavailable
        }
    }

    /// Marks an update or rollback as started directly from Settings after inline confirmation.
    pub fn start_running(&mut self) -> bool {
        if self.running {
            return false;
        }
        self.running = true;
        self.chip_focused = false;
        self.confirmation = None;
        true
    }

    /// Cancels bar chip focus and any open confirmation prompt.
    pub fn cancel(&mut self) {
        self.chip_focused = false;
        self.confirmation = None;
        self.confirm_held = false;
    }

    /// Drops only the bar chip focus (e.g. when a higher overlay opens).
    pub const fn blur_chip(&mut self) {
        self.chip_focused = false;
    }

    /// Handles a controller disconnect without discarding an automatic recovery prompt.
    pub fn controller_disconnected(&mut self) {
        self.chip_focused = false;
        self.confirm_held = false;
        self.held_buttons.clear();
        if !matches!(
            self.confirmation,
            Some(PendingConfirmation::Rollback {
                after_failed_starts: true,
                ..
            })
        ) {
            self.confirmation = None;
        }
    }

    /// Records completion of an update or rollback script.
    pub fn finish(&mut self, outcome: Outcome) {
        self.running = false;
        self.chip_focused = false;
        self.confirmation = None;
        self.confirm_held = false;
        match outcome {
            Outcome::Updated { installed, previous } | Outcome::RolledBack { installed, previous } => {
                self.info.available = None;
                if installed.is_some() {
                    self.info.installed = installed;
                }
                if previous.is_some() {
                    self.info.previous = previous;
                }
            }
            Outcome::Failed(_) => {}
        }
    }

    /// Consumes the release of a button that was pressed while the bar chip or confirmation owned the pad.
    pub fn consume_held_release(&mut self, name: ButtonName, state: ButtonState) -> bool {
        if self.confirmation.is_none() && !self.chip_focused && self.held_buttons.contains(&name) {
            if state == ButtonState::Released {
                self.held_buttons.retain(|held| *held != name);
            }
            return true;
        }
        false
    }

    fn track_button(&mut self, name: ButtonName, state: ButtonState) {
        match state {
            ButtonState::Pressed if !self.held_buttons.contains(&name) => {
                self.held_buttons.push(name);
            }
            ButtonState::Released => self.held_buttons.retain(|held| *held != name),
            ButtonState::Pressed => {}
        }
    }

    fn fresh_confirm(&mut self, family: PadFamily, name: ButtonName, state: ButtonState) -> bool {
        if name == family.button(Role::Confirm) {
            let was_held = core::mem::replace(&mut self.confirm_held, state == ButtonState::Pressed);
            return !was_held && state == ButtonState::Pressed;
        }
        state == ButtonState::Pressed
    }

    /// Handles a pad button while [`PendingConfirmation`] is open.
    pub fn confirmation_button(
        &mut self,
        family: PadFamily,
        name: ButtonName,
        state: ButtonState,
    ) -> ConfirmationAction {
        if self.confirmation.is_none() {
            return ConfirmationAction::Consumed;
        }
        self.track_button(name, state);
        if !self.fresh_confirm(family, name, state) {
            return ConfirmationAction::Consumed;
        }
        if name == family.button(Role::Back) {
            self.cancel();
            return ConfirmationAction::Cancelled;
        }
        if name == family.button(Role::Confirm) {
            let pending = self.confirmation.take();
            self.chip_focused = false;
            self.running = true;
            return match pending {
                Some(PendingConfirmation::Update { .. }) => ConfirmationAction::ApplyUpdate,
                Some(PendingConfirmation::Rollback { .. }) => ConfirmationAction::ApplyRollback,
                None => ConfirmationAction::Consumed,
            };
        }
        ConfirmationAction::Consumed
    }

    /// Handles D-pad focus, Confirm and Back on the bar's `Update` chip.
    pub fn bar_button(
        &mut self,
        family: PadFamily,
        level: Level,
        name: ButtonName,
        state: ButtonState,
    ) -> BarAction {
        if self.available().is_none() {
            self.chip_focused = false;
            return BarAction::Ignored;
        }
        if !self.chip_focused {
            if level == Level::Columns
                && state == ButtonState::Pressed
                && matches!(
                    name,
                    ButtonName::DpadDown | ButtonName::DpadLeft | ButtonName::DpadRight
                )
            {
                self.chip_focused = true;
                self.track_button(name, state);
                return BarAction::Consumed;
            }
            return BarAction::Ignored;
        }
        self.track_button(name, state);
        if state == ButtonState::Released {
            return BarAction::Consumed;
        }
        if name == family.button(Role::Back) || name == ButtonName::DpadUp {
            self.chip_focused = false;
        } else if name == family.button(Role::Confirm) {
            let _outcome = self.request_update(true);
        }
        BarAction::Consumed
    }

    /// Overlay text for the active confirmation prompt, using the connected pad's labels.
    #[must_use]
    pub fn confirmation_text(&self, family: PadFamily) -> Option<String> {
        let confirm = family.label(family.button(Role::Confirm));
        let back = family.label(family.button(Role::Back));
        match self.confirmation.as_ref()? {
            PendingConfirmation::Update { version } => Some(format!(
                "UPDATE TO {}?\n{confirm} CONFIRM / {back} CANCEL",
                version.to_ascii_uppercase()
            )),
            PendingConfirmation::Rollback {
                version,
                after_failed_starts: false,
            } => Some(format!(
                "PREVIOUS VERSION {}?\n{confirm} CONFIRM / {back} CANCEL",
                version.to_ascii_uppercase()
            )),
            PendingConfirmation::Rollback {
                version,
                after_failed_starts: true,
            } => Some(format!(
                "REPEATED FAILED STARTS\nPREVIOUS VERSION {}?\n{confirm} CONFIRM / {back} CANCEL",
                version.to_ascii_uppercase()
            )),
        }
    }
}
