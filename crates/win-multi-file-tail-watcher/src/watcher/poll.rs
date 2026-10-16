// Copyright (c) 2026 Mike Grier
//! `poll`: one poll at a time, run on the thread pool (TW-D-5).

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
use windows_threadpool_sys::work::ThreadpoolWork;

use super::Watcher;
use super::engine::{Engine, RunOutcome};
use crate::PollError;

#[cfg(test)]
mod tests;

// No poll is in flight.
const IDLE: u8 = 0;
// A poll has been started and its poll-done entry is not written: running, or waiting for room.
const RUNNING: u8 = 1;
// The poll-done entry is being written. Whether the poll is over is known once the write is.
const FINISHING: u8 = 2;

/// Whether a poll is in flight.
///
/// The state is not a flag because of one ordering. A caller who has read poll-done must be able to
/// start the next poll at once, so the gate has to be open before that entry can be seen, yet a
/// poll-done the queue refuses is not the end of the poll. `FINISHING` is the state in between:
/// a `poll` that finds it waits for the write's outcome, which is a few instructions away.
#[derive(Debug)]
pub(super) struct PollGate(AtomicU8);

impl PollGate {
    pub(super) const fn new() -> Self {
        Self(AtomicU8::new(IDLE))
    }

    /// Take the gate, or say a poll is in flight.
    fn begin(&self) -> bool {
        loop {
            match self
                .0
                .compare_exchange(IDLE, RUNNING, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return true,
                Err(FINISHING) => std::thread::yield_now(),
                Err(_) => return false,
            }
        }
    }

    /// Give back a gate taken by a `poll` that could not submit.
    fn abandon(&self) {
        self.0.store(IDLE, Ordering::Release);
    }

    /// The poll-done entry is about to be written.
    pub(super) fn finishing(&self) {
        self.0.store(FINISHING, Ordering::Release);
    }

    /// The poll-done entry was written: the poll is over.
    pub(super) fn finished(&self) {
        self.0.store(IDLE, Ordering::Release);
    }

    /// The poll-done entry was refused: the poll is still in flight.
    pub(super) fn unfinished(&self) {
        self.0.store(RUNNING, Ordering::Release);
    }
}

/// The thread-pool objects that run the poll, made by the first `poll`: a work item that starts it
/// and a wait on the resume doorbell that continues it after a stop.
///
/// Both are drained when it drops, which the objects would do anyway but which also discharges the
/// obligation the thread-pool crate reports for an undrained one.
pub(super) struct Poller {
    work: ThreadpoolWork,
    // Shared with the work item's callback, which arms it when a poll stops.
    wait: Arc<ThreadpoolWait>,
}

impl fmt::Debug for Poller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Poller")
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        // The work first: it is what arms the wait, so nothing arms it after the wait is drained.
        self.work.stop_and_drain();
        self.wait.stop_and_drain();
    }
}

// What both callbacks run: the engine, behind a lock so that a resume and the start of the next
// poll cannot overlap, against a view of the shared state that owns no thread-pool object.
struct PollRun<T: Clone> {
    view: Watcher<T>,
    engine: Mutex<Engine<T>>,
}

impl<T: Clone> PollRun<T> {
    fn run(&self) -> RunOutcome {
        self.engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .run(&self.view)
    }
}

impl<T: Clone + Send + 'static> Watcher<T> {
    /// Start a poll on the thread pool and return at once.
    ///
    /// A poll copies the roster, reads each file's length, and writes what changed to the
    /// submission queue, then a poll-done entry. [`doorbell`](Self::doorbell) is rung when entries
    /// are ready, when the queue is full, and when poll-done is written.
    ///
    /// One poll is in flight at a time: from this call until its poll-done entry is written. A
    /// poll that has stopped on a full queue is still in flight, and continues from where it
    /// stopped when the caller drains and rings [`resume_doorbell`](Self::resume_doorbell) with
    /// [`ring_resume`](Self::ring_resume). A caller who has read poll-done may call `poll` again at
    /// once.
    ///
    /// # Errors
    ///
    /// [`PollError::InFlight`] when a poll is in flight. [`PollError::Io`] when the thread-pool
    /// work cannot be created, which only the first call does.
    pub fn poll(&self) -> Result<(), PollError> {
        if !self.shared.gate.begin() {
            return Err(PollError::InFlight);
        }
        match self.poller() {
            Ok(poller) => {
                poller.work.submit();
                Ok(())
            }
            Err(error) => {
                self.shared.gate.abandon();
                Err(error)
            }
        }
    }

    // Only the call that holds the gate gets here, so there is one initialiser.
    fn poller(&self) -> Result<&Poller, PollError> {
        if let Some(poller) = self.poller.get() {
            return Ok(poller);
        }
        // The callbacks run against their own view of the shared state, one that owns no
        // thread-pool object: they must not own the objects they run on.
        let view = Self::view(Arc::clone(&self.shared));
        let run = Arc::new(PollRun {
            engine: Mutex::new(Engine::new(&view)),
            view,
        });

        // A duplicate of the resume doorbell's event, which the wait owns and closes; the caller
        // rings the original and the event is the same one.
        let event = self.shared.resume.duplicate_event()?;
        // SAFETY: an event is a waitable object the pool supports, not a mutex, and the duplicate
        // is owned by nothing else.
        let target = unsafe { WaitableHandle::assume_waitable(event) };
        let resumed = Arc::clone(&run);
        // Resets before it retries and arms again only afterwards, so a ring that lands in between
        // is seen by the arming: a manual-reset event that is already rung wakes the wait at once.
        let wait = Arc::new(ThreadpoolWait::new(
            target,
            move |activation| {
                resumed.view.reset_resume();
                if resumed.run() == RunOutcome::Stopped {
                    activation.rearm(None);
                }
            },
            None,
        )?);

        let parking = Arc::clone(&wait);
        let work = ThreadpoolWork::new(
            move || {
                if run.run() == RunOutcome::Stopped {
                    parking.arm(None);
                }
            },
            None,
        )?;
        Ok(self.poller.get_or_init(|| Poller { work, wait }))
    }

    fn view(shared: Arc<super::Shared<T>>) -> Self {
        Self {
            shared,
            poller: OnceLock::new(),
        }
    }
}
