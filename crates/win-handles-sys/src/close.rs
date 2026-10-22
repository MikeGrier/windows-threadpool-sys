// Copyright (c) 2026 Mike Grier
//! Closing a handle, and learning whether it closed: `CloseHandle`.

use std::io;
use std::os::windows::io::{IntoRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{CloseHandle, FALSE};

/// Closes `handle`, and reports what `CloseHandle` returned.
///
/// Dropping an `OwnedHandle` calls `CloseHandle` and has no way to say whether it worked. This is the same call with
/// the result kept, for a caller that needs to know: the page says only that a failure is a zero return with extended
/// error information from `GetLastError`.
///
/// The handle is taken whether or not the call succeeds, and is not given back. The page does not say what state a
/// handle is in after a failed close, and a handle that is not open cannot be given to a caller to drop, since dropping
/// it would close a value that may by then be another handle's. A handle protected from close
/// ([`HandleFlags::PROTECT_FROM_CLOSE`](crate::HandleFlags::PROTECT_FROM_CLOSE)), measured with no debugger attached,
/// failed to close and stayed open (WH-D-5), so it stays open for the life of the process: clear the flag with
/// [`set_handle_information`](crate::set_handle_information) first if the close is meant to succeed.
///
/// The page lists what `CloseHandle` closes, and says not to use it for a socket, for a handle from `FindFirstFile`, or
/// for a registry key: of the last it says that the call "does not close the handle to the registry key, but does not
/// return an error to indicate this failure", so a handle that is not one of the listed kinds is not made one by being
/// held in an `OwnedHandle`. Under a debugger the page says the call raises an exception for "a handle value that is not
/// valid or a pseudo-handle value", which an `OwnedHandle` does not hold.
///
/// # Example
///
/// ```
/// use std::fs::File;
///
/// use win_handles_sys::close_handle;
///
/// let path = std::env::temp_dir().join(format!("wh-close-{}.tmp", std::process::id()));
/// let file = File::create(&path)?;
///
/// // A `File` is an `OwnedHandle` by `From`; the close is reported and not ignored.
/// close_handle(file.into())?;
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The error from `CloseHandle`. A handle protected from close was refused where it was measured, with
/// `ERROR_INVALID_HANDLE` (WH-D-5). No other failure is reachable from here: the handle is open, since an `OwnedHandle`
/// is.
pub fn close_handle(handle: OwnedHandle) -> io::Result<()> {
    let raw = handle.into_raw_handle();
    // SAFETY: `raw` is the open handle `handle` owned, which nothing else will now close.
    if unsafe { CloseHandle(raw) } == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
