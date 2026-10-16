// Copyright (c) 2026 Mike Grier
//! The reading layer on the thread pool (TW-D-15, TW-D-5): the engine run, with no thread of the
//! caller's, whenever something it waits for happens. Its names are working names until the layer's
//! own is chosen.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the layer is not public until its names are chosen"
    )
)]

use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use windows_ioring_sys::CompletionWake;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
use wtf_string::Wtf16Str;

use crate::read_engine::{ReadEngine, Tracked};
use crate::read_sq::ReadSq;
use crate::watcher::StopHandle;
use crate::{OpenError, PollError, Registration};

// Here and not in `tests/` because the layer is not public.
#[cfg(test)]
mod child_tests;
#[cfg(test)]
mod tests;

/// What every callback runs: the engine, and the first error it returned.
struct Core<T: Clone> {
    engine: Mutex<ReadEngine<T>>,
    // A request to stop reading and a request to shut the watcher down, both without the engine's
    // lock, which a running callback may hold for a step.
    stop: Arc<AtomicBool>,
    watcher_stop: StopHandle<Tracked<T>>,
    failure: Mutex<Option<io::Error>>,
    // How many times the engine has been driven, so a test can see that nothing drives it between
    // polls.
    #[cfg(test)]
    runs: std::sync::atomic::AtomicUsize,
}

impl<T: Clone> Core<T> {
    fn engine(&self) -> std::sync::MutexGuard<'_, ReadEngine<T>> {
        // Nothing under the lock leaves the engine half-changed, so a poisoned one is consistent.
        self.engine.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run the engine once. The doorbells that woke this are reset by the run, before it looks.
    fn drive(&self) {
        #[cfg(test)]
        self.runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let result = self.engine().run();
        if let Err(error) = result {
            // Kept for the owner: the state is consistent and a later wake runs the engine again.
            self.failure
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert(error);
        }
    }
}

/// The reading engine, run on the thread pool.
///
/// Four things run it, each a thread-pool wait that calls the engine and then watches again, and
/// each watching an event that is rung and is reset by the run it causes: the watcher's doorbell
/// (entries to read), a read completing (the ring's own event), a buffer returned to an empty pool,
/// and the client's resume doorbell (room in its SQ). Between polls none of those events is rung, so
/// nothing runs, though the waits stay armed: an idle wait wakes no thread (TW-D-5).
///
/// Dropping it stops and drains every wait before the engine is released.
pub(crate) struct ReadLayer<T: Clone + Send + 'static> {
    // Field order is drop order, after `Drop::drop` has drained the waits: the ring's wake goes
    // first, and the engine, which the callbacks share, last.
    wake: CompletionWake,
    waits: [ThreadpoolWait; 3],
    sq: Arc<ReadSq<T>>,
    core: Arc<Core<T>>,
}

impl<T: Clone + Send + 'static> ReadLayer<T> {
    /// Run `engine` on the thread pool.
    ///
    /// # Errors
    ///
    /// When an event cannot be duplicated, a wait cannot be made, or the ring cannot attach its
    /// completion event.
    pub(crate) fn new(engine: ReadEngine<T>) -> io::Result<Self> {
        let sq = engine.shared_sq();
        let stop = engine.stop_flag();
        let watcher_stop = engine.watcher().stop_handle();
        let events = [
            engine.watcher().doorbell().try_clone_to_owned()?,
            engine.pool().doorbell().try_clone_to_owned()?,
            sq.resume_doorbell().try_clone_to_owned()?,
        ];
        let core = Arc::new(Core {
            engine: Mutex::new(engine),
            stop,
            watcher_stop,
            failure: Mutex::new(None),
            #[cfg(test)]
            runs: std::sync::atomic::AtomicUsize::new(0),
        });
        let [doorbell, pool, resume] = events;
        let waits = [
            watch(&core, doorbell)?,
            watch(&core, pool)?,
            watch(&core, resume)?,
        ];
        // Built under the engine's lock, which the setup signal's first call waits for.
        let on_wake = Arc::clone(&core);
        let wake = core
            .engine()
            .attach_completion_wake(move || on_wake.drive())?;
        Ok(Self {
            wake,
            waits,
            sq,
            core,
        })
    }

    /// Register the file at `path`, as [`ReadEngine::register`] does.
    ///
    /// # Errors
    ///
    /// As [`Watcher::register`](crate::Watcher::register).
    pub(crate) fn register(
        &self,
        path: &Wtf16Str,
        offset: u64,
        value: T,
    ) -> Result<Registration<Tracked<T>>, OpenError> {
        self.core.engine().register(path, offset, value)
    }

    /// Start a poll of the watcher, as [`Watcher::poll`](crate::Watcher::poll) does. What it finds
    /// is read, and arrives on [`read_sq`](Self::read_sq), with no further call.
    ///
    /// # Errors
    ///
    /// As [`Watcher::poll`](crate::Watcher::poll).
    pub(crate) fn poll(&self) -> Result<(), PollError> {
        self.core.engine().watcher().poll()
    }

    /// The SQ the client drains, with the doorbells it waits and rings on.
    pub(crate) fn read_sq(&self) -> &ReadSq<T> {
        &self.sq
    }

    /// Stop for good, without blocking and as often as asked: no poll starts, a poll running stops at
    /// its next step, no read is planned or submitted, reads in flight complete and are delivered,
    /// and every poll-done is still delivered (TW-D-9). Registering and dropping registrations go on
    /// working. Dropping the layer is what waits.
    pub(crate) fn shutdown(&self) {
        self.core.stop.store(true, Ordering::Release);
        self.core.watcher_stop.request();
        // Wakes the engine, which has nothing else to tell it, so it abandons what it planned and
        // delivers the poll-dones it owes.
        self.sq.ring_resume();
    }

    /// The first error the ring returned to a run, if one has, taken. The layer goes on.
    pub(crate) fn take_failure(&self) -> Option<io::Error> {
        self.core
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl<T: Clone + Send + 'static> Drop for ReadLayer<T> {
    fn drop(&mut self) {
        // Requested first, so a poll still running stops at its next step, no read is submitted
        // from here on, and what is in flight is all the ring's rundown waits for.
        self.shutdown();
        // Nothing arms a wait after construction but the wait's own callback, which the drain
        // suppresses, so each is idle when this returns.
        for wait in &self.waits {
            wait.stop_and_drain();
        }
        let _ = &self.wake;
    }
}

/// A wait on `event` that drives the engine and then watches again. The event is reset by the run,
/// so watching again does not spin.
fn watch<T: Clone + Send + 'static>(
    core: &Arc<Core<T>>,
    event: OwnedHandle,
) -> io::Result<ThreadpoolWait> {
    // SAFETY: an event is a waitable object the pool supports, not a mutex, and the duplicate is
    // owned by nothing else.
    let target = unsafe { WaitableHandle::assume_waitable(event) };
    let core = Arc::clone(core);
    let wait = ThreadpoolWait::new(
        target,
        move |activation| {
            core.drive();
            activation.rearm(None);
        },
        None,
    )?;
    wait.arm(None);
    Ok(wait)
}
