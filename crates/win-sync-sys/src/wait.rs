// Copyright (c) 2026 Mike Grier
//! Waiting on one kernel object: `WaitForSingleObject`.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

/// How a wait that did not fail ended.
///
/// Non-exhaustive: the alertable wait ends a third way, and joins this when it is added
/// ([WS-1+.1](../CHECKLIST.md)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WaitOutcome {
    /// The object was signalled, and the wait has taken whatever a signalled object gives: an
    /// auto-reset event is unsignalled again, a mutex is owned by the waiting thread, a semaphore's
    /// count is one lower.
    Signalled,
    /// The object is a mutex whose owner ended without releasing it. The waiting thread owns it
    /// now, and whatever it protected may be inconsistent. No other kind of object ends a wait
    /// this way, but the wait is over any handle, so a caller that cannot rule the handle out
    /// cannot ignore it.
    Abandoned,
    /// The timeout passed with the object not signalled.
    TimedOut,
}

/// Wait for `object` to be signalled, for up to `timeout`: `None` is no limit, and `Duration::ZERO`
/// only asks whether it is signalled now.
///
/// **A satisfied wait changes the object**, as the Win32 call does: it resets an auto-reset event,
/// acquires a mutex and decrements a semaphore. The wait is not alertable, so a queued APC does
/// not end it.
///
/// A timeout is rounded **up** to whole milliseconds, so a wait for a nonzero time is never a poll.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a timeout the call cannot express: `INFINITE` is `u32::MAX`
/// milliseconds, so the longest finite wait is one millisecond less, about 49.7 days. It is refused
/// rather than shortened, since a wait that ends early has given a wrong answer. Otherwise the
/// error from `GetLastError` when the wait fails, such as access denied for a handle without
/// `SYNCHRONIZE` access.
pub fn wait_for_single_object(
    object: impl AsHandle,
    timeout: Option<Duration>,
) -> io::Result<WaitOutcome> {
    let milliseconds = milliseconds(timeout)?;
    // SAFETY: a borrowed handle is open for the borrow's lifetime, and the wait ends within it.
    let code = unsafe { WaitForSingleObject(object.as_handle().as_raw_handle(), milliseconds) };
    outcome(code)
}

/// A timeout as the call takes it: whole milliseconds rounded up, `INFINITE` for none.
fn milliseconds(timeout: Option<Duration>) -> io::Result<u32> {
    let Some(timeout) = timeout else {
        return Ok(INFINITE);
    };
    let rounded_up = timeout.as_nanos().div_ceil(1_000_000);
    match u32::try_from(rounded_up) {
        // `INFINITE` is a value of the same parameter, so a finite wait may not take it.
        Ok(milliseconds) if milliseconds != INFINITE => Ok(milliseconds),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a wait of this many milliseconds cannot be expressed: the longest finite wait is \
             u32::MAX - 1",
        )),
    }
}

/// What the call returned, as an outcome or an error.
fn outcome(code: u32) -> io::Result<WaitOutcome> {
    match code {
        WAIT_OBJECT_0 => Ok(WaitOutcome::Signalled),
        WAIT_ABANDONED => Ok(WaitOutcome::Abandoned),
        WAIT_TIMEOUT => Ok(WaitOutcome::TimedOut),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        // A non-alertable wait on one object returns none of the others; named so the next reader
        // does not have to wonder, and not assumed away.
        other => Err(io::Error::other(format!(
            "WaitForSingleObject returned {other:#x}"
        ))),
    }
}

#[cfg(test)]
mod tests;
