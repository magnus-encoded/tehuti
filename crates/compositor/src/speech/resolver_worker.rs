//! Runs the resolver off the compositor loop and tracks which utterance waits on it.
//!
//! The loop submits a job and goes on dispatching. The worker thread blocks on the
//! transport and sends one [`Reply`] per job back through a channel the loop drains.
//! [`Pending`] holds the one utterance still waiting; a reply for any other job is late
//! and is dropped.

use core::panic::AssertUnwindSafe;
use core::time::Duration;
use std::{sync::mpsc, time::Instant};

/// How long an utterance may be held before it dictates as before.
pub const HELD_LIMIT: Duration = Duration::from_secs(1);

use tehuti_core::speech::resolver::Answer;

use super::resolver::{Resolver, Transport};

/// One question for the model.
#[derive(Debug)]
pub struct Job {
    pub number: u64,
    pub transcript: String,
    pub commands: Vec<String>,
}

/// The model's answer to one job; `None` when it failed, was refused or ran late.
#[derive(Debug)]
pub struct Reply {
    pub number: u64,
    pub answer: Option<Answer>,
}

/// The worker thread's handle. Dropping it ends the thread once its queue is empty.
#[derive(Debug)]
pub struct Worker {
    jobs: mpsc::Sender<Job>,
}

impl Worker {
    /// Starts the thread; `deliver` runs on it for each reply and returns whether the
    /// receiving side is still there.
    ///
    /// # Errors
    /// The thread could not be started.
    pub fn spawn<T, D>(resolver: Resolver<T>, deliver: D) -> std::io::Result<Self>
    where
        T: Transport + Send + 'static,
        D: Fn(Reply) -> bool + Send + 'static,
    {
        let (jobs, queue) = mpsc::channel::<Job>();
        let _handle = std::thread::Builder::new()
            .name("speech-resolver".into())
            .spawn(move || {
                for job in queue {
                    let answer = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        resolver.ask(true, &job.transcript, &job.commands)
                    }))
                    .unwrap_or_else(|_| {
                        tracing::warn!("speech resolver panicked; dictating as before");
                        None
                    });
                    if !deliver(Reply {
                        number: job.number,
                        answer,
                    }) {
                        return;
                    }
                }
            })?;
        Ok(Self { jobs })
    }

    /// Queues a job without waiting; false when the thread is gone.
    #[must_use]
    pub fn submit(&self, job: Job) -> bool {
        self.jobs.send(job).is_ok()
    }
}

/// An utterance waiting for its answer.
#[derive(Debug, PartialEq, Eq)]
pub struct Waiting {
    pub transcript: String,
    /// The focused application and title the utterance was spoken to.
    pub target: (Option<String>, String),
    pub commands: Vec<String>,
}

/// The single utterance whose answer is awaited.
#[derive(Debug, Default)]
pub struct Pending {
    next: u64,
    waiting: Option<(u64, Waiting, Instant)>,
}

impl Pending {
    /// Starts waiting on a new utterance. Returns its job number and the utterance it
    /// superseded, whose answer will now be dropped.
    pub fn begin(&mut self, waiting: Waiting) -> (u64, Option<Waiting>) {
        self.next = self.next.wrapping_add(1);
        let superseded = self
            .waiting
            .replace((self.next, waiting, Instant::now()))
            .map(|(_, old, _)| old);
        (self.next, superseded)
    }

    /// The utterance a reply belongs to; `None` when the reply is late for a superseded
    /// or already finished utterance.
    pub fn finish(&mut self, number: u64) -> Option<Waiting> {
        if self
            .waiting
            .as_ref()
            .is_some_and(|(current, _, _)| *current == number)
        {
            self.waiting.take().map(|(_, waiting, _)| waiting)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn is_waiting(&self) -> bool {
        self.waiting.is_some()
    }

    /// Gives up the waiting utterance, for example when the worker is gone.
    pub fn abandon(&mut self) -> Option<Waiting> {
        self.waiting.take().map(|(_, waiting, _)| waiting)
    }

    /// The waiting utterance when it has been held longer than `limit` at `now`.
    pub fn stale(&mut self, now: Instant, limit: Duration) -> Option<Waiting> {
        let held_too_long = self
            .waiting
            .as_ref()
            .is_some_and(|(_, _, since)| now.saturating_duration_since(*since) > limit);
        if held_too_long { self.abandon() } else { None }
    }
}

/// Everything the compositor needs to hold an unmatched utterance for the model.
#[derive(Debug)]
pub struct Link {
    pub(crate) worker: Worker,
    pub(crate) pending: Pending,
    pub(crate) opt_in: super::resolver::OptIn,
    pub(crate) log: Option<super::resolver::TurnLog>,
    /// Set while a held utterance is routed again, so it is not held twice.
    pub(crate) rerouting: bool,
}

impl Link {
    #[must_use]
    pub fn new(
        worker: Worker,
        opt_in: super::resolver::OptIn,
        log: Option<super::resolver::TurnLog>,
    ) -> Self {
        Self {
            worker,
            pending: Pending::default(),
            opt_in,
            log,
            rerouting: false,
        }
    }
}
