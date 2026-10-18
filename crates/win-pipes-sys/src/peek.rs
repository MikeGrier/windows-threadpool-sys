// Copyright (c) 2026 Mike Grier
//! Looking at the data in a pipe without taking it: `PeekNamedPipe`.
//!
//! Read 2026-10-18 from the call's page (`ms.date` 12/05/2018).

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::ptr;

use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

use crate::transact::output_len;

#[cfg(test)]
mod tests;

/// What `PeekNamedPipe` reported, field for field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PeekedPipe {
    /// `lpBytesRead`: "the number of bytes read from the pipe" into the buffer, which are not removed
    /// from the pipe.
    pub bytes_read: u32,
    /// `lpTotalBytesAvail`: "the total number of bytes available to be read from the pipe".
    pub total_bytes_available: u32,
    /// `lpBytesLeftThisMessage`: "the number of bytes remaining in this message", which the page says
    /// "will be zero for byte-type named pipes or for anonymous pipes". It is the call's value, as
    /// given; `WP-D-5` in the crate's design notes records what was observed for an anonymous pipe.
    pub bytes_left_this_message: u32,
}

/// Copies data from a pipe into `buffer` without removing it, and reports what is in the pipe:
/// `PeekNamedPipe`.
///
/// `buffer` is `lpBuffer` with `nBufferSize` its length, which the page says "can be NULL if no data
/// is to be read": `None` passes a null buffer and a size of zero, and asks only for the counts. A
/// buffer longer than `u32::MAX` is passed as `u32::MAX` bytes long, so the call reads no more than
/// that, and [`PeekedPipe::bytes_read`] says how much it did.
///
/// `pipe` is "a handle to a named pipe instance ... or ... the read end of an anonymous pipe" and
/// "must have `GENERIC_READ` access to the pipe". The page says that "the data is read in the mode
/// specified with `CreateNamedPipe`", so that a handle changed to byte-read mode afterwards is still
/// peeked in message mode; that for a byte-read handle the call "reads all available bytes up to the
/// size specified in `nBufferSize`" and for a message-read one "the next message in the pipe", of which
/// a message larger than the buffer gives "`TRUE` after reading the specified number of bytes", with
/// the rest counted in `bytes_left_this_message`. "The function always returns immediately in a
/// single-threaded application, even if there is no data in the pipe", the wait mode "has no effect on
/// the function", and "the function can block thread execution the same way any I/O function can when
/// called on a synchronous handle in a multi-threaded application"; "to avoid this condition, use a
/// pipe handle created for asynchronous I/O".
///
/// # Example
///
/// ```
/// use std::fs::File;
/// use std::io::Write;
///
/// use win_pipes_sys::{create_pipe, peek_named_pipe};
///
/// let pipe = create_pipe(0, None)?;
/// File::from(pipe.write).write_all(b"hello")?;
///
/// let mut buffer = [0u8; 3];
/// let peeked = peek_named_pipe(&pipe.read, Some(&mut buffer))?;
/// assert_eq!(&buffer, b"hel");
/// assert_eq!(peeked.bytes_read, 3);
/// assert_eq!(peeked.total_bytes_available, 5);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own error, which the page does not enumerate.
pub fn peek_named_pipe(pipe: &impl AsHandle, buffer: Option<&mut [u8]>) -> io::Result<PeekedPipe> {
    let (buffer_pointer, buffer_size) = match buffer {
        Some(buffer) => (
            buffer.as_mut_ptr().cast::<c_void>(),
            output_len(buffer.len()),
        ),
        None => (ptr::null_mut(), 0),
    };
    let mut peeked = PeekedPipe::default();

    // SAFETY: the handle is open for the borrow; `buffer_pointer` is null with a size of zero, or
    // is to `buffer`, which is valid for `buffer_size` bytes (never more than its length) for the
    // call; and the three counts are to a local that outlives it.
    let ok = unsafe {
        PeekNamedPipe(
            pipe.as_handle().as_raw_handle(),
            buffer_pointer,
            buffer_size,
            &mut peeked.bytes_read,
            &mut peeked.total_bytes_available,
            &mut peeked.bytes_left_this_message,
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(peeked)
}
