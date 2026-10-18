// Copyright (c) 2026 Mike Grier
//! Waiting for an instance of a named pipe to be available: `WaitNamedPipeW`, and the time-out both it
//! and `CallNamedPipeW` take.
//!
//! Read 2026-10-18 from the two pages (`ms.date` 08/05/2022 for `WaitNamedPipeW`, 04/19/2021 for
//! `CallNamedPipeW`).

use std::ffi::OsStr;
use std::io;

use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::System::Pipes::{
    NMPWAIT_NOWAIT, NMPWAIT_USE_DEFAULT_WAIT, NMPWAIT_WAIT_FOREVER, WaitNamedPipeW,
};

use crate::name::wide_name;

#[cfg(test)]
mod tests;

/// The time-out of `WaitNamedPipeW` and `CallNamedPipeW`, `nTimeOut`: "the number of milliseconds to
/// wait for the named pipe to be available", or one of the special values the pages list.
///
/// A newtype over the Win32 value, because the special values are in the same `u32` as the
/// milliseconds: [`NO_WAIT`](Self::NO_WAIT) is the value `1`, so a time-out of one millisecond given to
/// `CallNamedPipeW` is the call's "does not wait", and [`FOREVER`](Self::FOREVER) is `0xffffffff`.
/// `from_millis` does not move a number out of the way of either, and the default is
/// [`USE_DEFAULT`](Self::USE_DEFAULT).
///
/// # Example
///
/// ```
/// use win_pipes_sys::PipeTimeout;
///
/// assert_eq!(PipeTimeout::from_millis(250).millis(), 250);
/// assert_eq!(PipeTimeout::default(), PipeTimeout::USE_DEFAULT);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PipeTimeout(u32);

impl PipeTimeout {
    /// `NMPWAIT_USE_DEFAULT_WAIT` (`0x0`): "the time-out interval is the default value specified by
    /// the server process in the `CreateNamedPipe` function".
    pub const USE_DEFAULT: Self = Self(NMPWAIT_USE_DEFAULT_WAIT);

    /// `NMPWAIT_WAIT_FOREVER` (`0xffffffff`): "the function does not return until an instance of the
    /// named pipe is available".
    pub const FOREVER: Self = Self(NMPWAIT_WAIT_FOREVER);

    /// `NMPWAIT_NOWAIT` (`0x1`): "does not wait for the named pipe. If the named pipe is not
    /// available, the function returns an error." The `CallNamedPipeW` page lists it and the
    /// `WaitNamedPipeW` page does not.
    pub const NO_WAIT: Self = Self(NMPWAIT_NOWAIT);

    /// A time-out of `milliseconds` as the call takes it, with no value moved out of the way of the
    /// special ones above.
    #[must_use]
    pub const fn from_millis(milliseconds: u32) -> Self {
        Self(milliseconds)
    }

    /// The Win32 value.
    #[must_use]
    pub const fn millis(self) -> u32 {
        self.0
    }
}

/// Waits until an instance of the named pipe `name` is available for connection or the time-out
/// elapses: `WaitNamedPipeW`.
///
/// `name` "must include the name of the computer on which the server process is executing. A period
/// may be used for the servername if the pipe is local", as `\\.\pipe\pipename`; that is not checked
/// here, and a name with an interior NUL is [`io::ErrorKind::InvalidInput`] without calling.
///
/// Available means "the pipe's server process has a pending `ConnectNamedPipe` operation on the
/// pipe". "If no instances of the specified named pipe exist, the function returns immediately,
/// regardless of the time-out value", and "if the time-out interval expires, the function will fail
/// with the error `ERROR_SEM_TIMEOUT`". Success is not a promise: "a return value of TRUE indicates that
/// there is at least one instance of the pipe available. A subsequent `CreateFile` call to the pipe can
/// fail, because the instance was closed by the server or opened by another client."
///
/// # Example
///
/// ```
/// use win_pipes_sys::{PipeTimeout, wait_named_pipe};
///
/// // No instance of this pipe exists, so the call returns at once with the call's error.
/// let name = format!(r"\\.\pipe\win-pipes-sys-wait-doc-{}", std::process::id());
/// assert!(wait_named_pipe(&name, PipeTimeout::from_millis(10)).is_err());
/// ```
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a name with an interior NUL. Otherwise the call's own error,
/// among them `ERROR_SEM_TIMEOUT`.
pub fn wait_named_pipe(name: impl AsRef<OsStr>, timeout: PipeTimeout) -> io::Result<()> {
    let wide = wide_name(name.as_ref())?;
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let ok = unsafe { WaitNamedPipeW(wide.as_ptr(), timeout.0) };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
