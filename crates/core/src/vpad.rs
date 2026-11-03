//! Virtual pad overrides: plugin conditions set ahead of time, applied per input with no plugin round trip.

use tehuti_protocol::{ButtonName, ButtonState, PadOverride};

/// Where one physical button input goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Copy it to the virtual controller.
    Pass,
    /// Keep it from the application and send it to this plugin.
    Plugin(usize),
}

/// Override sets per plugin, plus the physical buttons held and the presses consumed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    overrides: Vec<(usize, Vec<PadOverride>)>,
    held: Vec<ButtonName>,
    consumed: Vec<(ButtonName, usize)>,
}

impl Filter {
    /// Replaces `plugin`'s overrides; an empty list clears them.
    pub fn set(&mut self, plugin: usize, overrides: Vec<PadOverride>) {
        self.clear(plugin);
        if !overrides.is_empty() {
            self.overrides.push((plugin, overrides));
        }
    }

    /// Drops `plugin`'s overrides, for example when it exits.
    pub fn clear(&mut self, plugin: usize) {
        self.overrides.retain(|(owner, _)| *owner != plugin);
    }

    /// Forgets held buttons and consumed presses, for a disconnect or a mode change.
    pub fn reset_held(&mut self) {
        self.held.clear();
        self.consumed.clear();
    }

    /// Whether a press of `button` is currently kept from the application.
    #[must_use]
    pub fn consumed(&self, button: ButtonName) -> bool {
        self.consumed.iter().any(|(held, _)| *held == button)
    }

    /// Decides one input. `eligible` lists plugins that may consume it, in priority order.
    /// A release always follows its press, whatever the overrides are by then.
    pub fn step(&mut self, button: ButtonName, state: ButtonState, eligible: &[usize]) -> Verdict {
        let owner = self
            .consumed
            .iter()
            .find(|(held, _)| *held == button)
            .map(|(_, plugin)| *plugin);
        match state {
            ButtonState::Released => {
                self.held.retain(|held| *held != button);
                self.consumed.retain(|(held, _)| *held != button);
                owner.map_or(Verdict::Pass, Verdict::Plugin)
            }
            ButtonState::Pressed => {
                if let Some(plugin) = owner {
                    return Verdict::Plugin(plugin);
                }
                if self.held.contains(&button) {
                    return Verdict::Pass;
                }
                let matched = eligible.iter().copied().find(|plugin| {
                    self.overrides.iter().any(|(setter, overrides)| {
                        setter == plugin
                            && overrides.iter().any(|entry| {
                                entry.button == button
                                    && entry.held.iter().all(|need| self.held.contains(need))
                            })
                    })
                });
                self.held.push(button);
                matched.map_or(Verdict::Pass, |plugin| {
                    self.consumed.push((button, plugin));
                    Verdict::Plugin(plugin)
                })
            }
        }
    }
}
