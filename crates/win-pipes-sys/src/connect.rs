// Copyright (c) 2026 Mike Grier
//! Connecting a client to a pipe instance and disconnecting it: `ConnectNamedPipe` and
//! `DisconnectNamedPipe`.
//!
//! Read 2026-10-18 from the two pages (`ms.date` 12/05/2018 for both). Everything the call reports
//! reaches the caller as it reported it: a result is the call's own outcome and an error is its own
//! code. Two of the call's outcomes are not errors although it returns zero for them, and are named
//! ([`ConnectOutcome::AlreadyConnected`], [`ConnectStart::Pending`]); every other zero is the error
//! code `GetLastError` gave.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, FALSE};
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};

#[cfg(test)]
mod tests;

/// How a connect that has finished came out.
///
/// The call returns zero for a client that was already connected, and says that "there is a good
/// connection between client and server, even though the function returns zero", so that is a result
/// here and not an error. What a nonzero return means depends on the handle's wait mode, which the
/// page states and this type does not decide: for a blocking handle it is "a client connects after the
/// function is called", and for one in nonblocking mode it is only that "the pipe is now available to
/// be connected to a new client".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectOutcome {
    /// The call returned nonzero.
    Succeeded,
    /// The call returned zero with `ERROR_PIPE_CONNECTED`: "a client connects before the function is
    /// called", in the interval after the instance was created or while it was still connected to a
    /// previous client that has not closed its handle.
    AlreadyConnected,
}

/// What an overlapped connect did when it was started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectStart {
    /// The call finished without pending, with the outcome it gave.
    Finished(ConnectOutcome),
    /// The call returned zero with `ERROR_IO_PENDING`: the operation is in progress, and completes
    /// through the `OVERLAPPED` it was given.
    Pending,
}

/// Waits for a client to connect to the pipe instance `pipe`: `ConnectNamedPipe` with no `OVERLAPPED`.
///
/// This is the synchronous form. The page says that "if `hNamedPipe` was not opened with
/// `FILE_FLAG_OVERLAPPED`, the function does not return until a client is connected or an error
/// occurs", and that if it was opened with it "the `lpOverlapped` parameter must not be NULL", and "if
/// `hNamedPipe` was opened with `FILE_FLAG_OVERLAPPED` and `lpOverlapped` is NULL, the function can
/// incorrectly report that the connect operation is complete". **No documented call reports how a
/// handle was opened, so this crate cannot check that `pipe` was not created overlapped, and the
/// caller must not give it one that was**: use
/// [`connect_named_pipe_overlapped`] for that. The page describes the consequence as an incorrect
/// report of completion, and says nothing of more.
///
/// **This blocks for as long as no client connects**, which is without limit; a pipe in nonblocking
/// mode returns at once instead.
///
/// # Example
///
/// ```
/// use std::fs::OpenOptions;
///
/// use win_pipes_sys::{
///     ConnectOutcome, NamedPipeOptions, PipeMode, PipeOpenMode, connect_named_pipe,
///     create_named_pipe,
/// };
///
/// let name = format!(r"\\.\pipe\win-pipes-sys-connect-doc-{}", std::process::id());
/// let options = NamedPipeOptions {
///     open_mode: PipeOpenMode::ACCESS_DUPLEX,
///     pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
///     max_instances: 1,
///     out_buffer_size: 4096,
///     in_buffer_size: 4096,
///     default_timeout_ms: 0,
/// };
/// let server = create_named_pipe(&name, &options, None)?;
///
/// // A client that connects before the call is reported as such, and is a good connection.
/// let _client = OpenOptions::new().read(true).write(true).open(&name)?;
/// assert_eq!(connect_named_pipe(&server)?, ConnectOutcome::AlreadyConnected);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own error, among them `ERROR_PIPE_LISTENING` for a pipe in nonblocking mode with no
/// client, and `ERROR_NO_DATA` for an instance whose previous client has closed its handle and that
/// the server has not disconnected, both as the page says.
pub fn connect_named_pipe(pipe: &impl AsHandle) -> io::Result<ConnectOutcome> {
    // SAFETY: the handle is open for the borrow, and no `OVERLAPPED` is passed, so nothing outlives
    // the call.
    let connected = unsafe { ConnectNamedPipe(pipe.as_handle().as_raw_handle(), ptr::null_mut()) };
    finished(connected)
}

/// Starts waiting for a client to connect to the pipe instance `pipe`, through `overlapped`:
/// `ConnectNamedPipe` with an `OVERLAPPED`.
///
/// This is the overlapped form. The page requires that, for a pipe created with
/// `FILE_FLAG_OVERLAPPED`, `overlapped` is not null and "point\[s\] to a valid `OVERLAPPED` structure",
/// and says it "should contain a handle to a manual-reset event object". The call returns at once.
/// When it is [`ConnectStart::Pending`] the operation completes later, through `overlapped`, however
/// the caller waits for that.
///
/// # Safety
///
/// `overlapped` must point to a valid `OVERLAPPED` that stays valid, and is not moved or reused, until
/// the operation has completed: the kernel writes to it when it does. `pipe` must be the server end of
/// a pipe instance created with [`PipeOpenMode::OVERLAPPED`](crate::PipeOpenMode::OVERLAPPED), and
/// must stay open until the operation has completed or been cancelled.
///
/// # Errors
///
/// The call's own error, which for a call that did not start an operation is not
/// `ERROR_IO_PENDING` or `ERROR_PIPE_CONNECTED`.
pub unsafe fn connect_named_pipe_overlapped(
    pipe: BorrowedHandle<'_>,
    overlapped: *mut OVERLAPPED,
) -> io::Result<ConnectStart> {
    // SAFETY: the handle is open for the borrow, and the caller guarantees `overlapped`.
    let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), overlapped) };
    if connected != FALSE {
        return Ok(ConnectStart::Finished(ConnectOutcome::Succeeded));
    }
    let error = io::Error::last_os_error();
    match u32::try_from(error.raw_os_error().unwrap_or(-1)) {
        Ok(ERROR_IO_PENDING) => Ok(ConnectStart::Pending),
        Ok(ERROR_PIPE_CONNECTED) => Ok(ConnectStart::Finished(ConnectOutcome::AlreadyConnected)),
        _ => Err(error),
    }
}

/// Disconnects the server end of the pipe instance `pipe` from its client: `DisconnectNamedPipe`.
///
/// The page says that if the client end is open the call "forces that end of the named pipe closed",
/// that the client "receives an error the next time it attempts to access the pipe" and "must still use
/// the `CloseHandle` function to close its end", that "any unread data in the pipe is discarded", and
/// that the server "must call `DisconnectNamedPipe` to disconnect a pipe handle from its previous
/// client before the handle can be connected to another client".
///
/// # Example
///
/// ```
/// use std::fs::OpenOptions;
/// use std::io::Write;
///
/// use win_pipes_sys::{
///     NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe, disconnect_named_pipe,
/// };
///
/// let name = format!(r"\\.\pipe\win-pipes-sys-disconnect-doc-{}", std::process::id());
/// let options = NamedPipeOptions {
///     open_mode: PipeOpenMode::ACCESS_DUPLEX,
///     pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
///     max_instances: 1,
///     out_buffer_size: 4096,
///     in_buffer_size: 4096,
///     default_timeout_ms: 0,
/// };
/// let server = create_named_pipe(&name, &options, None)?;
/// let mut client = OpenOptions::new().read(true).write(true).open(&name)?;
///
/// disconnect_named_pipe(&server)?;
///
/// // The client is forced off: its next access fails.
/// assert!(client.write_all(b"too late").is_err());
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own error.
pub fn disconnect_named_pipe(pipe: &impl AsHandle) -> io::Result<()> {
    // SAFETY: the handle is open for the borrow.
    let disconnected = unsafe { DisconnectNamedPipe(pipe.as_handle().as_raw_handle()) };
    if disconnected == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A synchronous connect's return, read for the outcome it names.
fn finished(connected: i32) -> io::Result<ConnectOutcome> {
    if connected != FALSE {
        return Ok(ConnectOutcome::Succeeded);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
        return Ok(ConnectOutcome::AlreadyConnected);
    }
    Err(error)
}
