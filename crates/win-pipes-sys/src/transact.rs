// Copyright (c) 2026 Mike Grier
//! A write and a read in one operation: `TransactNamedPipe` in its two forms, and `CallNamedPipeW`,
//! which makes the whole exchange on a name.
//!
//! Read 2026-10-18 from the two pages (`ms.date` 12/05/2018 for `TransactNamedPipe`, 04/19/2021 for
//! `CallNamedPipeW`).

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_MORE_DATA, FALSE};
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Pipes::{CallNamedPipeW, TransactNamedPipe};

use crate::SynchronousPipe;
use crate::name::wide_name;
use crate::wait::PipeTimeout;

#[cfg(test)]
mod tests;

/// What a transaction returned: the bytes the read half put in the output buffer, and whether the
/// message had more in it than the buffer held.
///
/// The page says that for a message "longer than the buffer specified by the `nOutBufferSize`
/// parameter", the call "returns FALSE and `GetLastError` returns `ERROR_MORE_DATA`", which is the
/// call's way of saying the buffer is full and the message is not finished, and not that nothing was
/// done. It is carried here as `more_data` and not as an error, as `AlreadyConnected` is for a connect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Transaction {
    /// `lpBytesRead`: the number of bytes read from the pipe into the output buffer.
    pub bytes_read: usize,
    /// Whether the call reported `ERROR_MORE_DATA`. For [`transact_named_pipe`] "the remainder of the
    /// message can be read by a subsequent call to `ReadFile`, `ReadFileEx`, or `PeekNamedPipe`"; for
    /// [`call_named_pipe`] "the remainder of the message is discarded, because `CallNamedPipe` closes
    /// the handle to the pipe before returning".
    pub more_data: bool,
}

/// How an overlapped transaction began.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransactStart {
    /// The call returned nonzero: the transaction finished before the call returned. The byte count
    /// is in the overlapped result, as for an operation that pended.
    Finished,
    /// The call returned zero with `ERROR_MORE_DATA`: the reply is longer than the output buffer. The
    /// bytes it did read and the rest of the message are as for [`Transaction::more_data`].
    MoreData,
    /// The call returned zero with `ERROR_IO_PENDING`: "the event object is set to the nonsignaled
    /// state before `TransactNamedPipe` returns, and it is set to the signaled state when the
    /// transaction has finished".
    Pending,
}

/// The length of an input buffer as the call takes it. A buffer the call could be told only part of
/// would write only part of it, so a length that does not fit is [`io::ErrorKind::InvalidInput`].
pub(crate) fn input_len(len: usize) -> io::Result<u32> {
    u32::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "an input buffer longer than u32::MAX bytes",
        )
    })
}

/// The length of an output buffer as the call takes it. A buffer the call is told is shorter than it
/// is can only be read into less, so a length that does not fit is `u32::MAX`, and the byte count the
/// call returns says how much it read.
pub(crate) fn output_len(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// Writes `input` to the pipe and reads a message into `output` in one operation, waiting for it:
/// `TransactNamedPipe` with no `OVERLAPPED`.
///
/// `pipe` is "a handle to the named pipe returned by the `CreateNamedPipe` or `CreateFile` function"
/// or "a handle to an anonymous pipe". The page says the call "fails if the server did not create the
/// pipe as a message-type pipe or if the pipe handle is not in message-read mode", that "a byte mode
/// pipe handle can be changed to message-read mode with the `SetNamedPipeHandleState` function", and
/// that "if a client is running on the same machine as the server and uses the `\\.\pipe\pipename`
/// format to open the pipe, the pipe is opened in byte mode". "The maximum guaranteed size of a named
/// pipe transaction is 64 kilobytes."
///
/// `output` longer than `u32::MAX` is passed as `u32::MAX` long, so the call reads no more than that. An
/// `input` longer than `u32::MAX` is [`io::ErrorKind::InvalidInput`], because the call cannot be told
/// to write part of it.
///
/// The page says that on a handle opened with `FILE_FLAG_OVERLAPPED` and no `OVERLAPPED`, "the
/// function can incorrectly report that the operation is complete", and a transaction reported
/// complete while its read is outstanding would let go of `output` while Windows still writes into it.
/// A [`SynchronousPipe`] was not opened with the flag, which is why this takes one; for a handle that
/// was, use [`transact_named_pipe_overlapped`], and for one that is not a `SynchronousPipe`,
/// [`transact_named_pipe_unchecked`].
///
/// # Example
///
/// ```no_run
/// use win_pipes_sys::{SynchronousPipe, transact_named_pipe};
/// # fn client(pipe: SynchronousPipe) -> std::io::Result<()> {
/// let mut reply = [0u8; 64];
/// let done = transact_named_pipe(&pipe, b"request", &mut reply)?;
/// assert!(!done.more_data);
/// let _reply = &reply[..done.bytes_read];
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for an `input` longer than `u32::MAX`. Otherwise the call's own
/// error, other than `ERROR_MORE_DATA`, which is [`Transaction::more_data`].
pub fn transact_named_pipe(
    pipe: &SynchronousPipe,
    input: &[u8],
    output: &mut [u8],
) -> io::Result<Transaction> {
    // SAFETY: a `SynchronousPipe` was not opened with `FILE_FLAG_OVERLAPPED`.
    unsafe { transact_named_pipe_unchecked(pipe.as_handle(), input, output) }
}

/// [`transact_named_pipe`] for a handle that is not a [`SynchronousPipe`].
///
/// # Safety
///
/// `pipe` must not have been opened with `FILE_FLAG_OVERLAPPED`. The page says that on a handle that
/// was, with no `OVERLAPPED`, "the function can incorrectly report that the operation is complete":
/// this would then return, ending the borrow of `output`, while the read still writes into it. No
/// documented call reports how a handle was opened, so this cannot check it.
///
/// # Errors
///
/// As [`transact_named_pipe`].
pub unsafe fn transact_named_pipe_unchecked(
    pipe: BorrowedHandle<'_>,
    input: &[u8],
    output: &mut [u8],
) -> io::Result<Transaction> {
    let input_size = input_len(input.len())?;
    let mut bytes_read = 0u32;

    // SAFETY: the handle is open for the borrow and, by this function's contract, was not opened
    // overlapped, so a null `OVERLAPPED` makes the call wait; `input` is valid for `input_size` bytes
    // and `output` for `output_len` (never more than its length) for the call, and `bytes_read` is a
    // local that outlives it.
    let ok = unsafe {
        TransactNamedPipe(
            pipe.as_raw_handle(),
            input.as_ptr().cast::<c_void>(),
            input_size,
            output.as_mut_ptr().cast::<c_void>(),
            output_len(output.len()),
            &mut bytes_read,
            ptr::null_mut(),
        )
    };
    transaction(ok, bytes_read)
}

/// Starts a transaction through `overlapped` and returns without waiting for it: `TransactNamedPipe`
/// with an `OVERLAPPED`.
///
/// The page requires that, for a handle opened with `FILE_FLAG_OVERLAPPED`, `overlapped` is not null
/// and "must point to a valid `OVERLAPPED` structure", which "should contain a manual-reset event
/// object", or the completion is delivered to a completion port the handle is associated with. The
/// byte count is not asked of the call, because the page says that for an overlapped operation it is
/// got "by calling `GetOverlappedResult`" or from the completion port, and `lpBytesRead` "can be NULL".
///
/// # Safety
///
/// `overlapped` must point to a valid `OVERLAPPED` that stays valid, and is not moved or reused, until
/// the operation has completed. `input` must be valid for reads of `input_size` bytes and `output`
/// for writes of `output_size` bytes, until the operation has completed. `pipe` must be a handle
/// opened with `FILE_FLAG_OVERLAPPED`, and must stay open until the operation has completed or been
/// cancelled.
///
/// # Errors
///
/// The call's own error, which for a call that did not start an operation is not `ERROR_IO_PENDING`
/// or `ERROR_MORE_DATA`.
pub unsafe fn transact_named_pipe_overlapped(
    pipe: BorrowedHandle<'_>,
    input: *const u8,
    input_size: u32,
    output: *mut u8,
    output_size: u32,
    overlapped: *mut OVERLAPPED,
) -> io::Result<TransactStart> {
    // SAFETY: the handle is open for the borrow, and the caller guarantees the buffers and
    // `overlapped`; `lpBytesRead` is null, which the page allows for an overlapped call.
    let ok = unsafe {
        TransactNamedPipe(
            pipe.as_raw_handle(),
            input.cast::<c_void>(),
            input_size,
            output.cast::<c_void>(),
            output_size,
            ptr::null_mut(),
            overlapped,
        )
    };
    if ok != FALSE {
        return Ok(TransactStart::Finished);
    }
    let error = io::Error::last_os_error();
    match u32::try_from(error.raw_os_error().unwrap_or(-1)) {
        Ok(ERROR_IO_PENDING) => Ok(TransactStart::Pending),
        Ok(ERROR_MORE_DATA) => Ok(TransactStart::MoreData),
        _ => Err(error),
    }
}

/// Connects to the message-type pipe `name`, writes `input`, reads a reply into `output`, and closes
/// the pipe: `CallNamedPipeW`.
///
/// The page says this "is equivalent to calling the `CreateFile` (or `WaitNamedPipe`, if `CreateFile`
/// cannot open the pipe immediately), `TransactNamedPipe`, and `CloseHandle` functions", with
/// "`GENERIC_READ | GENERIC_WRITE`, and an inherit handle flag of FALSE", that it "fails if the pipe
/// is a byte-type pipe", and that it waits "if an instance of the pipe is not available" for `timeout`
/// milliseconds. A reply "longer than `nOutBufferSize`" gives `ERROR_MORE_DATA`, "the remainder of the
/// message is discarded", and is [`Transaction::more_data`] here.
///
/// `name` with an interior NUL is [`io::ErrorKind::InvalidInput`] without calling, as are an `input`
/// longer than `u32::MAX`; an `output` longer than that is passed as `u32::MAX` long.
///
/// # Example
///
/// ```no_run
/// use win_pipes_sys::{PipeTimeout, call_named_pipe};
///
/// let mut reply = [0u8; 64];
/// let done = call_named_pipe(
///     r"\\.\pipe\some-message-pipe",
///     b"request",
///     &mut reply,
///     PipeTimeout::from_millis(1000),
/// )?;
/// let _reply = &reply[..done.bytes_read];
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] as above. Otherwise the call's own error, other than
/// `ERROR_MORE_DATA`.
pub fn call_named_pipe(
    name: impl AsRef<OsStr>,
    input: &[u8],
    output: &mut [u8],
    timeout: PipeTimeout,
) -> io::Result<Transaction> {
    let wide = wide_name(name.as_ref())?;
    let input_size = input_len(input.len())?;
    let mut bytes_read = 0u32;

    // SAFETY: `wide` is NUL-terminated and outlives the call; `input` is valid for `input_size`
    // bytes and `output` for `output_len` (never more than its length); and `bytes_read` is a local
    // that outlives it.
    let ok = unsafe {
        CallNamedPipeW(
            wide.as_ptr(),
            input.as_ptr().cast::<c_void>(),
            input_size,
            output.as_mut_ptr().cast::<c_void>(),
            output_len(output.len()),
            &mut bytes_read,
            timeout.millis(),
        )
    };
    transaction(ok, bytes_read)
}

/// The result of a synchronous transaction call: success, or `ERROR_MORE_DATA` as the page defines
/// it, or the call's error.
fn transaction(ok: i32, bytes_read: u32) -> io::Result<Transaction> {
    let more_data = if ok != FALSE {
        false
    } else {
        let error = io::Error::last_os_error();
        if u32::try_from(error.raw_os_error().unwrap_or(-1)) != Ok(ERROR_MORE_DATA) {
            return Err(error);
        }
        true
    };
    Ok(Transaction {
        bytes_read: bytes_read as usize,
        more_data,
    })
}
