//! Holds back a client commit whose dmabuf is not ready, so one stuck client
//! never stalls the output: its last good buffer stays on screen and every
//! other surface keeps drawing.

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use smithay::{
    reexports::wayland_server::{Resource as _, Weak, protocol::wl_surface::WlSurface},
    wayland::compositor::with_states,
    wayland::compositor::{Blocker, BlockerState},
};
use std::{
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

/// The blocker smithay holds a commit on.
#[derive(Debug, Clone)]
pub struct BufferBlocker(pub Arc<Gate>);

impl Blocker for BufferBlocker {
    fn state(&self) -> BlockerState {
        if self.0.ready(Instant::now()) {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}

/// The gate held on a surface, kept in the surface's own data so it is dropped with the surface.
#[derive(Debug, Default)]
pub struct Held(pub std::sync::Mutex<Option<Arc<Gate>>>);

/// Weak handles to surfaces that hold a gate; they keep nothing alive.
#[derive(Debug, Default)]
pub struct Waiting(Vec<Weak<WlSurface>>);

impl Waiting {
    pub fn push(&mut self, surface: &WlSurface) {
        self.0.push(surface.downgrade());
    }

    /// Forgets dead surfaces and surfaces whose buffer became ready, returning the latter.
    pub fn take_ready(&mut self, now: Instant) -> Vec<WlSurface> {
        let mut ready = Vec::new();
        self.0.retain(|weak| {
            let Ok(surface) = weak.upgrade() else {
                return false;
            };
            let done = with_states(&surface, |states| {
                states.data_map.get::<Held>().is_none_or(|held| {
                    let mut held = held.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let done = held.as_ref().is_none_or(|gate| gate.ready(now));
                    if done {
                        *held = None;
                    }
                    done
                })
            });
            if done {
                ready.push(surface);
            }
            !done
        });
        ready
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
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
        let blocker = BufferBlocker(Arc::new(Gate::new(vec![read], "viewer".into(), Instant::now())));
        assert_eq!(blocker.state(), BlockerState::Pending);
    }
}
