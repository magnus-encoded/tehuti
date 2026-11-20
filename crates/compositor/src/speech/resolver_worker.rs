//! Runs the resolver off the compositor loop and tracks which utterance waits on it.
//!
//! The loop submits a job and goes on dispatching. The worker thread blocks on the
//! transport and sends one [`Reply`] per job back through a channel the loop drains.
//! [`Pending`] holds the one utterance still waiting; a reply for any other job is late
//! and is dropped.

use std::sync::mpsc;

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
                    let answer = resolver.ask(true, &job.transcript, &job.commands);
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
    pub commands: Vec<String>,
}

/// The single utterance whose answer is awaited.
#[derive(Debug, Default)]
pub struct Pending {
    next: u64,
    waiting: Option<(u64, Waiting)>,
}

impl Pending {
    /// Starts waiting on a new utterance. Returns its job number and the utterance it
    /// superseded, whose answer will now be dropped.
    pub fn begin(&mut self, waiting: Waiting) -> (u64, Option<Waiting>) {
        self.next = self.next.wrapping_add(1);
        let superseded = self.waiting.replace((self.next, waiting)).map(|(_, old)| old);
        (self.next, superseded)
    }

    /// The utterance a reply belongs to; `None` when the reply is late for a superseded
    /// or already finished utterance.
    pub fn finish(&mut self, number: u64) -> Option<Waiting> {
        if self
            .waiting
            .as_ref()
            .is_some_and(|(current, _)| *current == number)
        {
            self.waiting.take().map(|(_, waiting)| waiting)
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
        self.waiting.take().map(|(_, waiting)| waiting)
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
