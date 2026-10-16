// Copyright (c) 2026 Mike Grier
//! `poll`: one poll at a time, run on the thread pool (TW-D-5).

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use windows_threadpool_sys::work::ThreadpoolWork;

use super::Watcher;
use super::engine::Engine;
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

/// The thread-pool work item that runs the poll, made by the first `poll`.
///
/// It is drained when it drops, which is what the work object would do anyway but which also
/// discharges the obligation the thread-pool crate reports for an undrained one.
pub(super) struct Poller(ThreadpoolWork);

impl fmt::Debug for Poller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Poller")
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.0.stop_and_drain();
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
    /// poll that has stopped on a full queue is still in flight. A caller who has read poll-done
    /// may call `poll` again at once.
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
                poller.0.submit();
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
        // The callback runs against its own view of the shared state, one that owns no work
        // object: it must not own the object it runs on.
        let view = Self::view(Arc::clone(&self.shared));
        let engine = Mutex::new(Engine::new(&view));
        let work = ThreadpoolWork::new(
            move || {
                engine
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .run(&view);
            },
            None,
        )?;
        Ok(self.poller.get_or_init(|| Poller(work)))
    }

    fn view(shared: Arc<super::Shared<T>>) -> Self {
        Self {
            shared,
            poller: OnceLock::new(),
        }
    }
}
