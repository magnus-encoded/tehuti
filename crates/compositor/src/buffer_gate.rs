//! Holds back a client commit whose dmabuf is not ready, so one stuck client
//! never stalls the output: its last good buffer stays on screen and every
//! other surface keeps drawing.

use core::sync::atomic::{AtomicBool, Ordering};
extern crate alloc;
use core::time::Duration;
use smithay::{
    reexports::wayland_server::{Resource as _, Weak, backend::ObjectId, protocol::wl_surface::WlSurface},
    wayland::compositor::with_states,
    wayland::compositor::{Blocker, BlockerState},
};
use std::{
    collections::{HashMap, VecDeque},
    os::fd::{AsFd, OwnedFd},
    sync::Arc,
    time::Instant,
};

/// How long a commit may wait before the stall is logged.
pub const STALL_AFTER: Duration = Duration::from_secs(1);

/// The planes of one client buffer, ready when every plane is readable.
#[derive(Debug)]
pub struct Gate {
    planes: Vec<OwnedFd>,
    client: String,
    since: Instant,
    reported: AtomicBool,
}

impl Gate {
    #[must_use]
    pub const fn new(planes: Vec<OwnedFd>, client: String, since: Instant) -> Self {
        Self {
            planes,
            client,
            since,
            reported: AtomicBool::new(false),
        }
    }

    /// Whether the client's writes to the buffer have finished. Logs once when
    /// the wait passes [`STALL_AFTER`].
    #[must_use]
    pub fn ready(&self, now: Instant) -> bool {
        let ready = self.planes.iter().all(|plane| {
            let mut fds = [rustix::event::PollFd::new(plane, rustix::event::PollFlags::IN)];
            rustix::event::poll(
                &mut fds,
                Some(&rustix::time::Timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            )
            .is_ok_and(|count| count > 0)
        });
        if !ready
            && now.saturating_duration_since(self.since) > STALL_AFTER
            && !self.reported.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(
                client = %self.client,
                waited = ?now.saturating_duration_since(self.since),
                "frame not shown: client buffer not ready; keeping its last good buffer"
            );
        }
        ready
    }

    #[must_use]
    pub fn client(&self) -> &str {
        &self.client
    }
}

/// The blocker smithay holds a commit on. It does not own the gate, so a commit
/// queued by smithay cannot keep the gate's fds alive after its surface is gone.
#[derive(Debug, Clone)]
pub struct BufferBlocker(pub alloc::sync::Weak<Gate>);

impl Blocker for BufferBlocker {
    fn state(&self) -> BlockerState {
        // The surface owns the gate; a gate that is gone has nothing left to wait for.
        if self.0.upgrade().is_none_or(|gate| gate.ready(Instant::now())) {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}

/// The gates of a surface's held commits, oldest first.
///
/// Each commit owns one gate, shared with that commit's blocker. Kept in the surface's own data, so a destroyed
/// surface or a dead client takes them (and their fds) along.
#[derive(Debug, Default)]
pub struct Held(pub std::sync::Mutex<VecDeque<Arc<Gate>>>);

impl Held {
    /// Adds the gate of a newly held commit.
    pub fn hold(&self, gate: Arc<Gate>) {
        self.lock().push_back(gate);
    }

    /// Drops every gate that is ready, whatever its position, and returns how many went.
    pub fn clear_ready(&self, now: Instant) -> usize {
        let mut gates = self.lock();
        let before = gates.len();
        gates.retain(|gate| !gate.ready(now));
        before.saturating_sub(gates.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Arc<Gate>>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Weak handles to surfaces that hold gates, one per surface; they keep nothing alive.
#[derive(Debug, Default)]
pub struct Waiting(HashMap<ObjectId, Weak<WlSurface>>);

impl Waiting {
    pub fn register(&mut self, surface: &WlSurface) {
        let _previous = self.0.insert(surface.id(), surface.downgrade());
    }

    /// Forgets dead surfaces and surfaces with nothing held, and returns the live
    /// surfaces where a gate became ready, so their commits can be re-evaluated.
    pub fn take_ready(&mut self, now: Instant) -> Vec<WlSurface> {
        let mut ready = Vec::new();
        self.0.retain(|_, weak| {
            let Ok(surface) = weak.upgrade() else {
                return false;
            };
            let (cleared, empty) = with_states(&surface, |states| {
                states
                    .data_map
                    .get::<Held>()
                    .map_or((0, true), |held| (held.clear_ready(now), held.is_empty()))
            });
            if cleared > 0 {
                ready.push(surface);
            }
            !empty
        });
        ready
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Weak handles to every held gate, so tests can check teardown dropped them.
    #[must_use]
    pub fn gate_probes(&self) -> Vec<alloc::sync::Weak<Gate>> {
        self.0
            .values()
            .filter_map(|weak| weak.upgrade().ok())
            .flat_map(|surface| {
                with_states(&surface, |states| {
                    states.data_map.get::<Held>().map_or_else(Vec::new, |held| {
                        held.lock().iter().map(Arc::downgrade).collect::<Vec<_>>()
                    })
                })
            })
            .collect()
    }
}

/// Duplicates the plane handles of a dmabuf so the gate outlives the buffer.
#[must_use]
pub fn planes_of(dmabuf: &smithay::backend::allocator::dmabuf::Dmabuf) -> Vec<OwnedFd> {
    dmabuf
        .handles()
        .filter_map(|handle| handle.as_fd().try_clone_to_owned().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn pipe() -> (OwnedFd, std::fs::File) {
        let (read, write) = rustix::pipe::pipe().expect("pipe");
        (read, std::fs::File::from(write))
    }

    #[test]
    fn a_buffer_nobody_finished_is_never_ready() {
        let (read, _write) = pipe();
        let start = Instant::now();
        let gate = Gate::new(vec![read], "viewer".into(), start);
        assert!(!gate.ready(start));
        assert!(!gate.ready(start + Duration::from_secs(60)));
    }

    #[test]
    fn a_finished_buffer_is_ready() {
        let (read, mut write) = pipe();
        write.write_all(b"x").expect("write");
        let gate = Gate::new(vec![read], "viewer".into(), Instant::now());
        assert!(gate.ready(Instant::now()));
    }

    #[test]
    fn one_unfinished_plane_holds_the_buffer() {
        let (done, mut write) = pipe();
        write.write_all(b"x").expect("write");
        let (stuck, _keep) = pipe();
        let gate = Gate::new(vec![done, stuck], "viewer".into(), Instant::now());
        assert!(!gate.ready(Instant::now()));
    }

    #[test]
    fn the_blocker_stays_pending_while_the_buffer_is_unfinished() {
        let (read, _write) = pipe();
        let gate = Arc::new(Gate::new(vec![read], "viewer".into(), Instant::now()));
        let blocker = BufferBlocker(Arc::downgrade(&gate));
        assert_eq!(blocker.state(), BlockerState::Pending);
        drop(gate);
        assert_eq!(
            blocker.state(),
            BlockerState::Released,
            "a dropped gate waits for nothing"
        );
    }
}
