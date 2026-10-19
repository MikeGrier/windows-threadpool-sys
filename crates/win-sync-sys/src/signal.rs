// Copyright (c) 2026 Mike Grier
//! Signalling one object and waiting on another in one step: `SignalObjectAndWait`.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::time::Duration;

use windows_sys::Win32::System::Threading::SignalObjectAndWait;

use crate::event::Event;
use crate::wait::{Alertable, WaitOutcome, milliseconds, outcome};

/// An object [`signal_object_and_wait`] can signal: an event, a semaphore or a mutex.
///
/// What a type implementing this promises is that signalling its handle is something the call can
/// do, so a handle that cannot signal is refused when it is passed, not when the call fails. The
/// wait-only handle ([`WaitOnlyEvent`](crate::WaitOnlyEvent)) does not implement it, and the
/// compiler says so:
///
/// ```compile_fail,E0277
/// use win_sync_sys::{Alertable, Event, ResetMode, signal_object_and_wait};
///
/// let ping = Event::new(ResetMode::Auto, false)?;
/// let waiter = ping.duplicate_wait_only()?;
/// let pong = Event::new(ResetMode::Auto, true)?;
/// // A wait-only handle cannot be signalled.
/// signal_object_and_wait(&waiter, &pong, None, Alertable::No)?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// It is not sealed: a handle that implements it and cannot in fact be signalled gets the error
/// from the call, not undefined behaviour.
pub trait Signalable: AsHandle {}

impl Signalable for Event {}

impl<T: Signalable + ?Sized> Signalable for &T {}

/// Signal `signal`, then wait for `wait`, as one step with nothing able to happen between the two:
/// `SignalObjectAndWait`.
///
/// It is one call to the kernel where a `set` followed by a wait is two, which is what Microsoft
/// offers it for: a handshake between two threads without the context switch the second call can
/// cost.
///
/// `signal` is set if it is an event, released once if it is a semaphore, and released if it is a
/// mutex the calling thread owns. A wait that times out or is alerted has **still signalled**: the
/// signal is not undone. The outcomes are those of a wait on one object, with `wait` changed as
/// [`wait_for_single_object`](crate::wait_for_single_object) describes.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a timeout the call cannot express, before anything is
/// signalled. Otherwise the error from `GetLastError`, and then **nothing was waited on and the
/// signal may or may not have been sent**: the call reports one failure for either half, so a
/// caller that needs to know checks the object. Access denied for a `signal` handle without the
/// right to modify it, or for a `wait` handle without `SYNCHRONIZE`; invalid handle for one that
/// is not an object the call can signal.
pub fn signal_object_and_wait(
    signal: impl Signalable,
    wait: impl AsHandle,
    timeout: Option<Duration>,
    alertable: Alertable,
) -> io::Result<WaitOutcome> {
    let milliseconds = milliseconds(timeout)?;
    // SAFETY: both handles are borrowed, so open for the call, and the wait ends within it.
    let code = unsafe {
        SignalObjectAndWait(
            signal.as_handle().as_raw_handle(),
            wait.as_handle().as_raw_handle(),
            milliseconds,
            alertable.as_bool(),
        )
    };
    outcome(code)
}

#[cfg(test)]
mod tests;
