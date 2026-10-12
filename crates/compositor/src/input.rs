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
}

/// A controller driven by code, for tests that run without devices.
#[derive(Debug, Default)]
pub struct SyntheticPad {
    pending: VecDeque<Event>,
}

impl SyntheticPad {
    pub fn push(&mut self, event: Event) {
        self.pending.push_back(event);
    }
}

impl InputSource for SyntheticPad {
    fn drain(&mut self) -> io::Result<Vec<Event>> {
        Ok(self.pending.drain(..).collect())
    }
}

/// Feeds every ready event from `source` through the core step and applies the resulting actions.
///
/// # Errors
/// The source failed to read.
pub fn route(compositor: &mut Compositor, source: &mut dyn InputSource) -> io::Result<()> {
    for event in source.drain()? {
        compositor.handle(&event);
    }
    Ok(())
}
