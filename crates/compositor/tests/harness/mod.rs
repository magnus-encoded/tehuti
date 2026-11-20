//! Test harness that serves Wayland clients without sleeping or reading the wall clock.
//!
//! Clients run on their own threads. The server side waits on the display socket and on a
//! wake pipe that each client thread writes when it ends or notifies, so a test never polls on a
//! timer. A client that stalls without ending or writing fails the test after [`STALL_LIMIT`] of
//! silence instead of hanging until the runner's timeout; that bound is a poll timeout, not a
//! clock read in the test.

#![expect(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    missing_debug_implementations,
    reason = "test helper: panics are the failure report and results are not optional"
)]

use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, mpsc},
    thread,
    time::Instant,
};

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tehuti::state::Compositor;
use tehuti_protocol::Event;

/// How long the server waits with no socket traffic and no wake before the test fails.
pub const STALL_LIMIT: Duration = Duration::from_secs(20);

/// How long a polled wait sleeps in `poll` between turns.
const POLL_STEP: Duration = Duration::from_millis(2);

/// The longest a test waits on a real subprocess before it fails.
pub const SUBPROCESS_LIMIT: Duration = Duration::from_secs(60);

/// A fake clock: the compositor reads no wall time, and time passes only when the test says so.
#[derive(Debug, Default)]
pub struct FakeClock {
    elapsed: Duration,
}

impl FakeClock {
    /// Switches `compositor` to the driver clock and starts at zero.
    pub fn install(compositor: &mut Compositor) -> Self {
        compositor.use_driver_clock();
        Self::default()
    }

    /// Lets `span` pass: ticks the compositor, then releases the keys that have come due.
    pub fn advance(&mut self, compositor: &mut Compositor, span: Duration) {
        self.elapsed = self.elapsed.saturating_add(span);
        compositor.handle(&Event::Tick { elapsed: span });
        compositor.release_due_keys();
    }

    /// Fake time that has passed since [`Self::install`].
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

/// Real elapsed time, for tests whose subject is a real subprocess that keeps its own clock.
#[derive(Debug)]
pub struct Stopwatch {
    start: Instant,
    lap: Instant,
}

impl Stopwatch {
    pub fn start() -> Self {
        let now = Instant::now();
        Self { start: now, lap: now }
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Time since the previous call to `lap` (or the start).
    pub fn lap(&mut self) -> Duration {
        let now = Instant::now();
        let passed = now.duration_since(self.lap);
        self.lap = now;
        passed
    }
}

/// Sends a value to the test body and, when made by [`Harness::channel`], wakes the server loop.
pub struct Notifier<T> {
    sender: mpsc::Sender<T>,
    wake: Option<Arc<UnixStream>>,
}

impl<T> Notifier<T> {
    /// Sends `value` to the receiver, then wakes the server loop. The wake write is best effort:
    /// its result is ignored, because the value is already queued and the loop rechecks it.
    ///
    /// # Errors
    ///
    /// Returns the value when the receiver has been dropped.
    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        let sent = self.sender.send(value);
        if let Some(mut wake) = self.wake.as_deref() {
            let _written = wake.write(&[1]);
        }
        sent
    }
}

/// A channel whose sends do not wake a server loop, for tests that poll the receiver themselves.
pub fn silent_channel<T>() -> (Notifier<T>, mpsc::Receiver<T>) {
    let (sender, receiver) = mpsc::channel();
    (Notifier { sender, wake: None }, receiver)
}

/// Runs the event loop until `done` holds. Bounded by [`SUBPROCESS_LIMIT`].
pub fn pump_until(
    event_loop: &mut EventLoop<'static, Compositor>,
    compositor: &mut Compositor,
    mut done: impl FnMut(&mut Compositor) -> bool,
) {
    let began = Instant::now();
    while !done(compositor) {
        assert!(
            began.elapsed() < SUBPROCESS_LIMIT,
            "subprocess did not finish within {SUBPROCESS_LIMIT:?}"
        );
        event_loop.dispatch(POLL_STEP, compositor).unwrap();
    }
}

/// Runs the event loop for `span` of real time, calling `each` after every dispatch.
pub fn pump_for(
    event_loop: &mut EventLoop<'static, Compositor>,
    compositor: &mut Compositor,
    span: Duration,
    mut each: impl FnMut(&mut Compositor),
) {
    let began = Instant::now();
    while began.elapsed() < span {
        event_loop.dispatch(POLL_STEP, compositor).unwrap();
        each(compositor);
    }
}

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

    /// A channel from a client thread to the test body whose sends wake the server loop.
    pub fn channel<T>(&self) -> (Notifier<T>, mpsc::Receiver<T>) {
        let (sender, receiver) = mpsc::channel();
        let wake = Some(Arc::new(self.wake_write.try_clone().unwrap()));
        (Notifier { sender, wake }, receiver)
    }

    /// Serves clients until `done` holds, blocking on socket readiness between turns.
    pub fn serve_until(
        &mut self,
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        mut done: impl FnMut() -> bool,
    ) {
        self.serve_turns(display, compositor, |_| done());
    }

    /// Like [`Self::serve_until`], and hands the compositor to `turn` after each dispatch so the
    /// test can act on it; `turn` returns whether the test is done.
    pub fn serve_turns(
        &mut self,
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        mut turn: impl FnMut(&mut Compositor) -> bool,
    ) {
        loop {
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            let done = turn(compositor);
            display.flush_clients().unwrap();
            if done {
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

    /// Like [`Self::serve_turns`], for tests that wait on a real subprocess whose output reaches
    /// the compositor through its own event loop, so no wake pipe fires. Waits are short polls;
    /// the test fails after [`SUBPROCESS_LIMIT`].
    pub fn serve_polled(
        &mut self,
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        mut turn: impl FnMut(&mut Compositor) -> bool,
    ) {
        let began = Instant::now();
        loop {
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            let done = turn(compositor);
            display.flush_clients().unwrap();
            if done {
                return;
            }
            assert!(
                began.elapsed() < SUBPROCESS_LIMIT,
                "subprocess did not finish within {SUBPROCESS_LIMIT:?}"
            );
            let _ready = self.poll_for(display, POLL_STEP);
        }
    }

    fn wait(&mut self, display: &mut Display<Compositor>) {
        let ready = self.poll_for(display, STALL_LIMIT);
        assert!(ready, "no client traffic, notice or end within {STALL_LIMIT:?}");
    }

    /// Waits up to `limit` for socket traffic or a wake, and drains the wake pipe.
    fn poll_for(&mut self, display: &mut Display<Compositor>, limit: Duration) -> bool {
        use rustix::event::{PollFd, PollFlags, poll};
        let backend = display.backend();
        let display_fd = backend.poll_fd();
        let mut fds = [
            PollFd::new(&display_fd, PollFlags::IN),
            PollFd::new(&self.wake_read, PollFlags::IN),
        ];
        let timeout = rustix::event::Timespec {
            tv_sec: limit.as_secs().try_into().unwrap(),
            tv_nsec: limit.subsec_nanos().into(),
        };
        let polled = poll(&mut fds, Some(&timeout));
        let ready = polled == Err(rustix::io::Errno::INTR) || polled.unwrap() > 0;
        let mut drained = [0_u8; 64];
        while matches!(self.wake_read.read(&mut drained), Ok(count) if count > 0) {}
        ready
    }
}
