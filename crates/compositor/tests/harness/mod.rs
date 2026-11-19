//! Test harness that serves Wayland clients without sleeping or reading the wall clock.
//!
//! Clients run on their own threads. The server side waits on the display socket and on a
//! wake pipe that each client thread writes when it ends, so a test never polls on a timer.

#![expect(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    reason = "test helper: panics are the failure report and results are not optional"
)]

use core::sync::atomic::{AtomicBool, Ordering};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::Arc,
    thread,
};

use smithay::reexports::wayland_server::Display;
use tehuti::state::Compositor;

/// A client thread that wakes the server loop when it ends, even by panic.
pub struct Client<T> {
    handle: thread::JoinHandle<T>,
    ended: Arc<AtomicBool>,
}

impl<T> Client<T> {
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// Returns the client's result; panics if the client panicked.
    pub fn join(self) -> T {
        self.handle.join().unwrap()
    }
}

/// Tells the server loop that a client has ended.
struct EndSignal {
    ended: Arc<AtomicBool>,
    pipe: UnixStream,
}

impl Drop for EndSignal {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::SeqCst);
        let _written = self.pipe.write(&[1]);
    }
}

/// The server's side of the wake pipe.
pub struct Harness {
    wake_read: UnixStream,
    wake_write: UnixStream,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    pub fn new() -> Self {
        let (wake_read, wake_write) = UnixStream::pair().unwrap();
        wake_read.set_nonblocking(true).unwrap();
        wake_write.set_nonblocking(true).unwrap();
        Self {
            wake_read,
            wake_write,
        }
    }

    /// Runs `body` on a thread whose end wakes [`Self::serve_until_ended`].
    pub fn client<T: Send + 'static>(&self, body: impl FnOnce() -> T + Send + 'static) -> Client<T> {
        let ended = Arc::new(AtomicBool::new(false));
        let signal = EndSignal {
            ended: ended.clone(),
            pipe: self.wake_write.try_clone().unwrap(),
        };
        let handle = thread::spawn(move || {
            let _signal = signal;
            body()
        });
        Client { handle, ended }
    }

    /// Serves clients until `done` holds, blocking on socket readiness between turns.
    pub fn serve_until(
        &mut self,
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        mut done: impl FnMut() -> bool,
    ) {
        loop {
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            display.flush_clients().unwrap();
            if done() {
                return;
            }
            self.wait(display);
        }
    }

    /// Serves clients until every one of them has ended.
    pub fn serve_until_ended<T>(
        &mut self,
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        clients: &[&Client<T>],
    ) {
        self.serve_until(display, compositor, || {
            clients.iter().copied().all(Client::has_ended)
        });
    }

    fn wait(&mut self, display: &mut Display<Compositor>) {
        use rustix::event::{PollFd, PollFlags, poll};
        let backend = display.backend();
        let display_fd = backend.poll_fd();
        let mut fds = [
            PollFd::new(&display_fd, PollFlags::IN),
            PollFd::new(&self.wake_read, PollFlags::IN),
        ];
        let polled = poll(&mut fds, None);
        if polled != Err(rustix::io::Errno::INTR) {
            polled.unwrap();
        }
        let mut drained = [0_u8; 64];
        while matches!(self.wake_read.read(&mut drained), Ok(count) if count > 0) {}
    }
}
