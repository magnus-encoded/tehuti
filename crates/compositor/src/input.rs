//! The input-source seam: every controller, real or synthetic, yields protocol events.

use std::{collections::VecDeque, io};

use tehuti_protocol::Event;

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
    let events = match source.drain() {
        Ok(events) => events,
        Err(error) => {
            compositor.speech_input(tehuti_core::speech::SpeechInput::Disconnected);
            compositor.controller_input(tehuti_core::controller::Input::Disconnected);
            return Err(error);
        }
    };
    for event in events {
        compositor.controller_input(tehuti_core::controller::Input::Event(event));
        if compositor.quit_requested() {
            break;
        }
    }
    Ok(())
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
