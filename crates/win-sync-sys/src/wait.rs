// Copyright (c) 2026 Mike Grier
//! Waiting on one kernel object: `WaitForSingleObject` and `WaitForSingleObjectEx`.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    FALSE, TRUE, WAIT_ABANDONED, WAIT_FAILED, WAIT_IO_COMPLETION, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject, WaitForSingleObjectEx};

/// Whether a wait can be ended early by an APC queued to the waiting thread.
///
/// An alertable wait runs every APC queued to the thread, and I/O completion routines, before it
/// returns, and then reports [`WaitOutcome::Alerted`]; a wait that is not alertable leaves them
/// queued. [`queue_user_apc`](crate::queue_user_apc) is what queues one.
///
/// **A pending APC wins over a signalled object.** On the Windows this was observed on, an
/// alertable wait with an APC already queued runs it and returns `Alerted` even when the object is
/// signalled, and leaves the object as it was; the next wait takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alertable {
    /// The wait ends only by its object, or its timeout.
    No,
    /// A queued APC ends the wait.
    Yes,
}

impl Alertable {
    /// The `BOOL` the Win32 calls take.
    pub(crate) fn as_bool(self) -> i32 {
        match self {
            Self::No => FALSE,
            Self::Yes => TRUE,
        }
    }
}

/// How a wait that did not fail ended.
///
/// Non-exhaustive, so a wait that can end another way can be added.
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
    /// An alertable wait was ended by an APC queued to the thread, which has run. Only a wait made
    /// [`Alertable::Yes`] ends this way. **The object was not waited for**: nothing was taken from
    /// it, so the caller waits again if it still needs it.
    Alerted,
}

/// Wait for `object` to be signalled, for up to `timeout`: `None` is no limit, and `Duration::ZERO`
/// only asks whether it is signalled now.
///
/// **A satisfied wait changes the object**, as the Win32 call does: it resets an auto-reset event,
/// acquires a mutex and decrements a semaphore. The wait is not alertable, so a queued APC does
/// not end it; [`wait_for_single_object_ex`] is the one an APC can end.
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

/// [`wait_for_single_object`], which an APC can end: `WaitForSingleObjectEx`.
///
/// With [`Alertable::No`] it is the same wait as [`wait_for_single_object`]. With
/// [`Alertable::Yes`] it returns [`WaitOutcome::Alerted`] when an APC queued to the calling thread
/// ran, without having taken anything from the object.
///
/// # Errors
///
/// As [`wait_for_single_object`].
pub fn wait_for_single_object_ex(
    object: impl AsHandle,
    timeout: Option<Duration>,
    alertable: Alertable,
) -> io::Result<WaitOutcome> {
    let milliseconds = milliseconds(timeout)?;
    // SAFETY: a borrowed handle is open for the borrow's lifetime, and the wait ends within it.
    let code = unsafe {
        WaitForSingleObjectEx(
            object.as_handle().as_raw_handle(),
            milliseconds,
            alertable.as_bool(),
        )
    };
    outcome(code)
}

/// A timeout as the call takes it: whole milliseconds rounded up, `INFINITE` for none.
pub(crate) fn milliseconds(timeout: Option<Duration>) -> io::Result<u32> {
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

/// What a wait on one object returned, as an outcome or an error.
pub(crate) fn outcome(code: u32) -> io::Result<WaitOutcome> {
    match code {
        WAIT_OBJECT_0 => Ok(WaitOutcome::Signalled),
        WAIT_ABANDONED => Ok(WaitOutcome::Abandoned),
        WAIT_TIMEOUT => Ok(WaitOutcome::TimedOut),
        WAIT_IO_COMPLETION => Ok(WaitOutcome::Alerted),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        // The call returns no other value; named so the next reader does not have to wonder, and
        // not assumed away.
        other => Err(io::Error::other(format!("a wait returned {other:#x}"))),
    }
}

#[cfg(test)]
mod tests;
