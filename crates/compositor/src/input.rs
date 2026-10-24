//! The input-source seam: every controller, real or synthetic, yields protocol events.

use std::{collections::VecDeque, io};

use tehuti_core::dispatch::ControllerChoice;
use tehuti_protocol::{ButtonName, ButtonState, Event};

use crate::state::Compositor;

/// Something that produces controller events for the core.
pub trait InputSource {
    /// Takes every event that is ready now, without blocking.
    ///
    /// # Errors
    /// The source is gone or unreadable; the caller drops it.
    fn drain(&mut self) -> io::Result<Vec<Event>>;

    /// Releases exclusive ownership of the controller.
    ///
    /// # Errors
    /// The device could not release its grab.
    fn release(&mut self) -> io::Result<()>;
}

/// A controller driven by code, for tests that run without devices.
#[derive(Debug, Default)]
pub struct SyntheticPad {
    pending: VecDeque<Event>,
    released: bool,
}

impl SyntheticPad {
    #[must_use]
    pub const fn is_released(&self) -> bool {
        self.released
    }

    pub fn push(&mut self, event: Event) {
        self.pending.push_back(event);
    }
}

impl InputSource for SyntheticPad {
    fn release(&mut self) -> io::Result<()> {
        self.released = true;
        Ok(())
    }

    fn drain(&mut self) -> io::Result<Vec<Event>> {
        Ok(self.pending.drain(..).collect())
    }
}

/// Feeds every ready event from `source` through the core step and applies the resulting actions.
///
/// # Errors
/// The source failed to read.
pub fn route(compositor: &mut Compositor, source: &mut dyn InputSource) -> io::Result<()> {
    route_counted(compositor, source).map(|_| ())
}

/// Routes pending input and returns the number of events read.
/// # Errors
/// The source failed to read.
pub fn route_counted(compositor: &mut Compositor, source: &mut dyn InputSource) -> io::Result<usize> {
    let events = match source.drain() {
        Ok(events) => events,
        Err(error) => {
            compositor.speech_input(tehuti_core::speech::SpeechInput::Disconnected);
            compositor.controller_input(tehuti_core::controller::Input::Disconnected);
            return Err(error);
        }
    };
    let count = events.len();
    for event in events {
        compositor.controller_input(tehuti_core::controller::Input::Event(event));
        if compositor.quit_requested() {
            break;
        }
    }
    Ok(count)
}

/// Releases the input source before stopping the event loop on a core quit request.
///
/// # Errors
/// The source could not release its grab.
pub fn finish_quit(
    compositor: &Compositor,
    source: &mut dyn InputSource,
    signal: &smithay::reexports::calloop::LoopSignal,
) -> io::Result<()> {
    if compositor.quit_requested() {
        let result = source.release();
        signal.stop();
        return result;
    }
    Ok(())
}

/// Drains every connected pad, removing only pads whose reads fail.
/// Returns merged events and the paths whose event registrations must be removed.
pub fn drain_pads<S: InputSource>(
    pads: &mut std::collections::BTreeMap<std::path::PathBuf, S>,
) -> (Vec<Event>, Vec<std::path::PathBuf>) {
    drain_pads_admitted(pads, |_, _, _| true)
}

/// Like [`drain_pads`], keeping only the events `admit` accepts.
pub fn drain_pads_admitted<S: InputSource>(
    pads: &mut std::collections::BTreeMap<std::path::PathBuf, S>,
    mut admit: impl FnMut(&std::path::Path, &S, &Event) -> bool,
) -> (Vec<Event>, Vec<std::path::PathBuf>) {
    let mut events = Vec::new();
    let mut removed = Vec::new();
    pads.retain(|path, pad| match pad.drain() {
        Ok(pending) => {
            events.extend(pending.into_iter().filter(|event| admit(path, pad, event)));
            true
        }
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "controller read failed; dropping it");
            removed.push(path.clone());
            false
        }
    });
    (events, removed)
}

/// Whether the pad named `pad` drives Tehuti: every pad under Auto, and every pad
/// while the chosen one is not connected, so a missing pad never locks the user out.
#[must_use]
pub fn chosen(choice: &ControllerChoice, connected: &[String], pad: &str) -> bool {
    match choice {
        ControllerChoice::Auto => true,
        ControllerChoice::Named(name) => name == pad || !connected.contains(name),
    }
}

/// Drops input from pads that are not chosen, except releases of buttons it let through.
#[derive(Debug, Default)]
pub struct PadGate {
    held: std::collections::BTreeMap<std::path::PathBuf, Vec<ButtonName>>,
}

impl PadGate {
    pub fn admit(&mut self, path: &std::path::Path, chosen: bool, event: &Event) -> bool {
        let Event::Button { name, state } = event else {
            return chosen;
        };
        let held = self.held.entry(path.to_path_buf()).or_default();
        let was_held = held.contains(name);
        if *state == ButtonState::Released {
            held.retain(|button| button != name);
            was_held || chosen
        } else {
            if chosen && !was_held {
                held.push(*name);
            }
            chosen
        }
    }

    /// Forgets pads that are gone.
    pub fn retain(&mut self, connected: impl Fn(&std::path::Path) -> bool) {
        self.held.retain(|path, _| connected(path));
    }
}
