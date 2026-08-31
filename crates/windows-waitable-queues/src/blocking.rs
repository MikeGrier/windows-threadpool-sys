// Copyright (c) Mike Grier.

//! The blocking receive loop, written once for every shape that has one.
//!
//! # Why this is not simply copied into each shape
//!
//! The loop below is not glue -- it *is* the arming protocol, the contract
//! recorded as [D-9](../../DESIGN-NOTES.md#d-9): drain, arm, and wait only if
//! arming blessed it, with the disconnection check placed between the arming
//! and the wait so a producer that vanished cannot leave a consumer parked.
//! Every step is load-bearing and the order is the whole correctness argument.
//!
//! A second shape spelling that sequence out again would be a second copy of a
//! rule, free to drift from the first and, worse, free to *look* verified while
//! only the copy was tested. This crate has already paid for that mistake once,
//! in a lost-wakeup test that exercised a hand-written duplicate of
//! `Consumer::arm` rather than the real one and so could not have noticed the
//! real one being reversed. So the protocol is stated here, and a shape binds
//! to it by implementing [`Parked`].
//!
//! # Why [`Parked`] is not one of the public capability traits
//!
//! The public traits describe what a caller may *ask of* a queue. [`Parked`]
//! describes what this module needs *from* a queue in order to park on it, and
//! the difference shows in [`Parked::finish`], which no caller should ever
//! reach for: it is meaningful only after disconnection has already been
//! observed, and the public [`Consumer`](crate::Consumer) surface deliberately
//! does not offer a method whose contract is a precondition nobody can check
//! from outside.

use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

use crate::error::{RecvError, RecvTimeoutError};

/// What a shape must offer for [`recv`] and [`recv_timeout`] to park on it.
pub(crate) trait Parked {
    /// The item type the shape carries.
    type Item;

    /// Takes the oldest item, or `None` if there is none right now.
    fn pop(&self) -> Option<Self::Item>;

    /// The last take before the end of the stream is reported.
    ///
    /// Separate from [`Parked::pop`] so a shape can name the step and a test
    /// can call it directly. It guards a real and narrow race: a producer may
    /// push *and then* drop in the window between this loop's first `pop` and
    /// its disconnection check, and reporting the disconnection without one
    /// last take would silently discard an item that was successfully sent.
    fn finish(&self) -> Option<Self::Item>;

    /// Clears the doorbell and reports whether waiting on it is safe.
    ///
    /// # Errors
    ///
    /// Whatever creating the doorbell reports.
    fn arm(&self) -> io::Result<bool>;

    /// Whether every producer is gone.
    fn is_disconnected(&self) -> bool;

    /// The doorbell to park on.
    ///
    /// # Errors
    ///
    /// Whatever creating the doorbell reports.
    fn doorbell(&self) -> io::Result<BorrowedHandle<'_>>;
}

/// Takes the oldest item, blocking until one arrives.
///
/// # Errors
///
/// [`RecvError::Disconnected`] once every producer is gone *and* the queue is
/// drained -- items pushed before the last producer dropped are still
/// delivered. [`RecvError::Io`] if the doorbell cannot be created or waited on.
pub(crate) fn recv<C: Parked>(consumer: &C) -> Result<C::Item, RecvError> {
    loop {
        if let Some(item) = consumer.pop() {
            return Ok(item);
        }
        if !consumer.arm()? {
            continue;
        }
        if consumer.is_disconnected() {
            return consumer.finish().ok_or(RecvError::Disconnected);
        }
        wait(consumer.doorbell()?, INFINITE)?;
    }
}

/// Takes the oldest item, blocking until one arrives or the deadline passes.
///
/// The timeout bounds the whole call, not each individual wait: a consumer
/// woken spuriously does not get a fresh budget.
///
/// # Errors
///
/// [`RecvTimeoutError::Timeout`] if the deadline passes with the queue still
/// empty, which is not a malfunction. Otherwise as [`recv`].
pub(crate) fn recv_timeout<C: Parked>(
    consumer: &C,
    timeout: Duration,
) -> Result<C::Item, RecvTimeoutError> {
    // `Instant + Duration` panics when the sum is not representable, and
    // `Duration::MAX` is a perfectly ordinary way to spell "effectively
    // forever". A library that panics on that is worse than one that blocks, so
    // an unrepresentable deadline degrades to the untimed wait it was asking
    // for rather than aborting the caller.
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return recv(consumer).map_err(|error| match error {
            RecvError::Disconnected => RecvTimeoutError::Disconnected,
            RecvError::Io(io) => RecvTimeoutError::Io(io),
        });
    };
    loop {
        if let Some(item) = consumer.pop() {
            return Ok(item);
        }
        if !consumer.arm()? {
            continue;
        }
        if consumer.is_disconnected() {
            return consumer.finish().ok_or(RecvTimeoutError::Disconnected);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RecvTimeoutError::Timeout);
        }
        // Saturating rather than wrapping: a duration longer than a `u32` of
        // milliseconds is roughly 49 days, and clamping it to that is a longer
        // wait than any caller meant, where truncating it would be a far
        // shorter one. The loop re-arms and waits again, so clamping costs an
        // extra turn and nothing else.
        let millis = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
        wait(consumer.doorbell()?, millis)?;
    }
}

/// Block on a doorbell handle, translating the Win32 result.
fn wait(handle: BorrowedHandle<'_>, millis: u32) -> io::Result<()> {
    // SAFETY: a live event handle borrowed for the duration of the call.
    let result = unsafe { WaitForSingleObject(handle.as_raw_handle(), millis) };
    match result {
        // A timeout is not an error here: the caller's loop re-checks its own
        // deadline and decides what a timeout means.
        WAIT_OBJECT_0 | WAIT_TIMEOUT => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}
