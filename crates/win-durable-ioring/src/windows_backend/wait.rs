// Copyright (c) 2026 Mike Grier
//! The Windows backend's notification, waited on (DI-D-55): the ring's completion event, as the
//! [`CompletionEvent`] the backend hands out.

use std::io;
use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
use std::time::Duration;

use win_sync_sys::{WaitOutcome, wait_for_single_object};

use durable_ioring::Waitable;

#[cfg(test)]
mod tests;

/// The ring's completion event, duplicated for the one that waits on it: the notification the
/// Windows backend hands out. A type of this crate and not a bare `OwnedHandle`, because
/// [`Waitable`] is the portable crate's trait and a handle is the standard library's, so neither
/// side could implement one for the other.
#[derive(Debug)]
pub struct CompletionEvent(OwnedHandle);

impl CompletionEvent {
    /// The event, taking the handle it is waited on through.
    #[must_use]
    pub fn new(handle: OwnedHandle) -> Self {
        Self(handle)
    }

    /// The handle, to wait on by other means.
    #[must_use]
    pub fn into_handle(self) -> OwnedHandle {
        self.0
    }
}

impl From<OwnedHandle> for CompletionEvent {
    fn from(handle: OwnedHandle) -> Self {
        Self(handle)
    }
}

impl AsHandle for CompletionEvent {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.0.as_handle()
    }
}

/// The ring's completion event is auto-reset (the ring crate's D-21), so a wait that sees it
/// signalled consumes the signal, and a raising before the first wait is seen by it: the ring crate
/// signals the event once as it attaches it (its D-20).
///
/// **One waiter per ring** is the caller's to keep, as the ring crate states it: a second wait on
/// the same event, or a duplicate of this handle waited on elsewhere, steals wakes.
impl Waitable for CompletionEvent {
    fn wait(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        match wait_for_single_object(&self.0, timeout)? {
            WaitOutcome::Signalled => Ok(true),
            WaitOutcome::TimedOut => Ok(false),
            // An event is never abandoned, which only a mutex can be. Not reached by a test here:
            // this takes any `OwnedHandle`, but making a mutex needs `unsafe`, which this crate
            // forbids. `win-sync-sys`' tests reach the ending in the function this calls. (A wait
            // failing is reached, with a duplicate that has no `SYNCHRONIZE` access.) The ending
            // is reported, not taken for a signal.
            other => Err(io::Error::other(format!(
                "the notification's wait ended as {other:?}"
            ))),
        }
    }
}
