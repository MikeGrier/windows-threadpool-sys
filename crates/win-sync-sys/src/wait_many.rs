// Copyright (c) 2026 Mike Grier
//! Waiting on several kernel objects at once: `WaitForMultipleObjectsEx`.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::ptr;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    FALSE, HANDLE, TRUE, WAIT_ABANDONED_0, WAIT_FAILED, WAIT_IO_COMPLETION, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::SystemServices::MAXIMUM_WAIT_OBJECTS;
use windows_sys::Win32::System::Threading::WaitForMultipleObjectsEx;

use crate::wait::{Alertable, WaitOutcome, milliseconds};

/// The most objects one wait takes: `MAXIMUM_WAIT_OBJECTS`.
///
/// More is refused, not split into several waits, since a wait on the first sixty-four is not a wait
/// on all of them; wait on groups of at most this many from different threads, or wait on a
/// smaller set that is signalled from the rest.
pub const MAXIMUM_OBJECTS: usize = MAXIMUM_WAIT_OBJECTS as usize;

/// How a [`wait_for_any`] that did not fail ended.
///
/// Non-exhaustive, as [`WaitOutcome`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WaitAnyOutcome {
    /// The object at this index was signalled, and the wait has taken what a signalled object gives
    /// from **that one object only**. Of several signalled at once the lowest index is reported,
    /// and the others are left as they were: an auto-reset event among them is still signalled.
    Signalled(usize),
    /// The object at this index is a mutex whose owner ended without releasing it. The waiting
    /// thread owns it now, and whatever it protected may be inconsistent. The lowest index of the
    /// signalled objects is reported here as it is for [`Signalled`](Self::Signalled).
    Abandoned(usize),
    /// The timeout passed with none of the objects signalled.
    TimedOut,
    /// An alertable wait was ended by an APC queued to the thread, which has run. **Nothing was
    /// taken from any object.**
    Alerted,
}

/// Wait until **any one** of `objects` is signalled, for up to `timeout`: `None` is no limit and
/// `Duration::ZERO` only asks. `WaitForMultipleObjectsEx` with `bWaitAll` false.
///
/// The objects are the borrowed handles of whatever the caller holds: a slice of `&Event`, of
/// `OwnedHandle`, or of `BorrowedHandle` for a mixture, made with `as_handle`.
///
/// **Only the signalled object reported has been changed** by the wait: an auto-reset event is
/// unsignalled again, a mutex owned, a semaphore's count one lower. The others are as they were,
/// signalled or not.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`], without waiting, for no objects, for more than
/// [`MAXIMUM_OBJECTS`], or for a timeout the call cannot express (see
/// [`wait_for_single_object`](crate::wait_for_single_object)). Otherwise the error from
/// `GetLastError` when the wait fails, such as access denied for a handle without `SYNCHRONIZE`
/// access, or invalid handle.
pub fn wait_for_any(
    objects: &[impl AsHandle],
    timeout: Option<Duration>,
    alertable: Alertable,
) -> io::Result<WaitAnyOutcome> {
    let (code, count) = wait(objects, false, timeout, alertable)?;
    any_outcome(code, count)
}

/// Wait until **all** of `objects` are signalled at once, for up to `timeout`: `None` is no limit and
/// `Duration::ZERO` only asks. `WaitForMultipleObjectsEx` with `bWaitAll` true.
///
/// **The wait is atomic**: it takes from the objects only when every one is signalled, and takes
/// from all of them together. A wait that times out has taken nothing, so an object that was
/// signalled during it is still signalled. **The same handle may not appear twice**: Windows
/// refuses it, and the error is the wait's. ([`wait_for_any`] tolerates it and reports the first
/// index.)
///
/// The outcomes are [`WaitOutcome`]'s: `Signalled` is every object signalled and taken from;
/// `Abandoned` is the same with at least one a mutex whose owner ended without releasing it, and
/// the waiting thread owns every mutex; `Alerted` is an alertable wait ended by an APC, with nothing
/// taken.
///
/// # Errors
///
/// As [`wait_for_any`].
pub fn wait_for_all(
    objects: &[impl AsHandle],
    timeout: Option<Duration>,
    alertable: Alertable,
) -> io::Result<WaitOutcome> {
    let (code, count) = wait(objects, true, timeout, alertable)?;
    all_outcome(code, count)
}

/// The call's return value and how many objects it was given.
fn wait(
    objects: &[impl AsHandle],
    wait_all: bool,
    timeout: Option<Duration>,
    alertable: Alertable,
) -> io::Result<(u32, usize)> {
    let milliseconds = milliseconds(timeout)?;
    let count = objects.len();
    if count == 0 || count > MAXIMUM_OBJECTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a wait takes from 1 to {MAXIMUM_OBJECTS} objects, and was given {count}"),
        ));
    }
    let mut handles: [HANDLE; MAXIMUM_OBJECTS] = [ptr::null_mut(); MAXIMUM_OBJECTS];
    for (slot, object) in handles.iter_mut().zip(objects) {
        *slot = object.as_handle().as_raw_handle();
    }
    // SAFETY: the first `count` entries are handles borrowed from `objects`, which are open for
    // the whole call, and `count` is at most the array's length; the call reads only those.
    let code = unsafe {
        WaitForMultipleObjectsEx(
            u32::try_from(count).unwrap_or(MAXIMUM_WAIT_OBJECTS),
            handles.as_ptr(),
            if wait_all { TRUE } else { FALSE },
            milliseconds,
            alertable.as_bool(),
        )
    };
    Ok((code, count))
}

/// What a wait for any returned, as an outcome or an error.
fn any_outcome(code: u32, count: usize) -> io::Result<WaitAnyOutcome> {
    let signalled = |first: u32| {
        code.checked_sub(first)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|&index| index < count)
    };
    if let Some(index) = signalled(WAIT_OBJECT_0) {
        return Ok(WaitAnyOutcome::Signalled(index));
    }
    if let Some(index) = signalled(WAIT_ABANDONED_0) {
        return Ok(WaitAnyOutcome::Abandoned(index));
    }
    match code {
        WAIT_TIMEOUT => Ok(WaitAnyOutcome::TimedOut),
        WAIT_IO_COMPLETION => Ok(WaitAnyOutcome::Alerted),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        // An index past the objects given, or a value the call does not return.
        other => Err(io::Error::other(format!(
            "a wait for any of {count} objects returned {other:#x}"
        ))),
    }
}

/// What a wait for all returned, as an outcome or an error.
///
/// For a wait for all the call reports the state of all of the objects, and the index in
/// `WAIT_OBJECT_0 + n` or `WAIT_ABANDONED_0 + n` is not one a caller can use, so it is not read.
fn all_outcome(code: u32, count: usize) -> io::Result<WaitOutcome> {
    let within =
        |first: u32| code >= first && usize::try_from(code - first).is_ok_and(|n| n < count);
    if within(WAIT_OBJECT_0) {
        return Ok(WaitOutcome::Signalled);
    }
    if within(WAIT_ABANDONED_0) {
        return Ok(WaitOutcome::Abandoned);
    }
    match code {
        WAIT_TIMEOUT => Ok(WaitOutcome::TimedOut),
        WAIT_IO_COMPLETION => Ok(WaitOutcome::Alerted),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        other => Err(io::Error::other(format!(
            "a wait for all of {count} objects returned {other:#x}"
        ))),
    }
}

#[cfg(test)]
mod tests;
