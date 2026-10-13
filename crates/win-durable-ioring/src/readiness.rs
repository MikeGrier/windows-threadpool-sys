// Copyright (c) 2026 Mike Grier
// Split from oracle.rs at a8947da8.
//! The readiness signal's harness check (DI-D-28), for a signal that is a Windows [`Event`]: push
//! into an empty queue and require the signal set, drain, and again. The portable oracle sees
//! entries, not wakes, so this runs beside it.

use std::io;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use durable_ioring::{DurableRing, EntryOf};
use win_sync_sys::Event;
use windows_threadpool_sys::wait::ThreadpoolWait;

#[cfg(test)]
mod tests;

/// Why [`check_readiness`] failed.
#[derive(Debug)]
pub enum ReadinessFailure {
    /// The first entry into an empty queue did not set the signal within the bound.
    NotSetForFirstEntry,
    /// An entry into the queue, emptied again after the first, did not set the signal within the
    /// bound: the signal was set once and not on a later empty-to-non-empty transition.
    NotSetAgain,
    /// Taking the signal, popping, or waiting failed.
    Io(io::Error),
}

impl From<io::Error> for ReadinessFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The readiness signal's harness check (DI-D-28): push into an empty queue and require the
/// signal set; drain; push again and require it set again. The oracle sees entries, not wakes, so
/// this runs beside it. For an implementation whose signal is an [`Event`], which
/// it waits on through the thread pool.
///
/// `push` must start exactly one operation whose completion becomes an entry. **Nothing may be in
/// flight when this is called**: it drains the queue and withdraws a stale signal first, and an
/// entry landing in between would have its wake withdrawn with it.
///
/// Returns every entry it popped, in order, so the caller can report them to its oracle.
///
/// # Errors
///
/// The first part of the check that failed.
pub fn check_readiness<D, P>(
    ring: &mut D,
    mut push: P,
    bound: Duration,
) -> Result<Vec<EntryOf<D>>, ReadinessFailure>
where
    D: DurableRing<Readiness = Event>,
    P: FnMut(&mut D),
{
    let mut entries = Vec::new();
    let signal = ring.readiness()?;
    drain(ring, &mut entries)?;
    signal.reset()?;

    let (woken, wakes) = mpsc::channel();
    let woken = Mutex::new(woken);
    let wait = ThreadpoolWait::new(
        signal.into(),
        move |_| {
            if let Ok(woken) = woken.lock() {
                let _ = woken.send(());
            }
        },
        None,
    )?;

    let mut steps = || -> Result<(), ReadinessFailure> {
        wait.arm(None);
        push(ring);
        wakes
            .recv_timeout(bound)
            .map_err(|_| ReadinessFailure::NotSetForFirstEntry)?;
        drain(ring, &mut entries)?;

        wait.arm(None);
        push(ring);
        wakes
            .recv_timeout(bound)
            .map_err(|_| ReadinessFailure::NotSetAgain)?;
        drain(ring, &mut entries)?;
        Ok(())
    };
    let outcome = steps();
    // Discharges the drain the wait owes, on every path.
    wait.stop_and_drain();
    outcome.map(|()| entries)
}

/// Pop until the queue is empty.
fn drain<D: DurableRing>(ring: &mut D, into: &mut Vec<EntryOf<D>>) -> io::Result<()> {
    while let Some(entry) = ring.pop()? {
        into.push(entry);
    }
    Ok(())
}
