// Copyright (c) 2026 Mike Grier
//! The Windows backend's notification, waited on (DI-D-55): the ring's completion event, as the
//! `OwnedHandle` the backend hands out.

use std::io;
use std::os::windows::io::OwnedHandle;
use std::time::Duration;

use win_sync_sys::{WaitOutcome, wait_for_single_object};

use crate::pump::Waitable;

#[cfg(test)]
mod tests;

/// The ring's completion event is auto-reset (the ring crate's D-21), so a wait that sees it
/// signalled consumes the signal, and a raising before the first wait is seen by it: the ring crate
/// signals the event once as it attaches it (its D-20).
///
/// **One waiter per ring** is the caller's to keep, as the ring crate states it: a second wait on
/// the same event, or a duplicate of this handle waited on elsewhere, steals wakes.
impl Waitable for OwnedHandle {
    fn wait(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        match wait_for_single_object(&*self, timeout)? {
            WaitOutcome::Signalled => Ok(true),
            WaitOutcome::TimedOut => Ok(false),
            // An event is never abandoned, which only a mutex can be. Not reached by a test here:
            // this takes any `OwnedHandle`, but making a mutex needs `unsafe`, which this crate
            // forbids, and the wait failing needs a handle without `SYNCHRONIZE` access, which
            // `CreateFile` never gives a file. `win-sync-sys`' tests reach both in the function this
            // calls. The ending is reported, not taken for a signal.
            other => Err(io::Error::other(format!(
                "the notification's wait ended as {other:?}"
            ))),
        }
    }
}
