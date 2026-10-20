// Copyright (c) 2026 Mike Grier
//! Asking a pipe who is at each end of it: `GetNamedPipeClientProcessId`,
//! `GetNamedPipeClientSessionId`, `GetNamedPipeServerProcessId`, `GetNamedPipeServerSessionId` and
//! `GetNamedPipeClientComputerNameW`.
//!
//! Read 2026-10-18 from the five pages (`ms.date` 12/05/2018 for the four identifier calls and 08/05/2022
//! for `GetNamedPipeClientComputerNameW`). Each takes "a handle to an instance of a named pipe", which
//! "must be created by the `CreateNamedPipe` function"; whether a handle from another source is refused is
//! the call's, and is not checked here.

use std::ffi::OsString;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::{FALSE, HANDLE};
use windows_sys::core::BOOL;

use crate::name::from_wide_until_nul;
use windows_sys::Win32::System::Pipes::{
    GetNamedPipeClientComputerNameW, GetNamedPipeClientProcessId, GetNamedPipeClientSessionId,
    GetNamedPipeServerProcessId, GetNamedPipeServerSessionId,
};

#[cfg(test)]
mod tests;

/// One of the four calls that write a `ULONG` through a pointer.
type IdentifierCall = unsafe extern "system" fn(HANDLE, *mut u32) -> BOOL;

/// Calls `call` on `pipe` and returns the value it wrote. The one place the four identifier calls'
/// shared shape is written.
fn identifier(pipe: &impl AsHandle, call: IdentifierCall) -> io::Result<u32> {
    let mut value = 0u32;
    // SAFETY: the handle is open for the borrow and `value` is a local that outlives the call.
    let ok = unsafe { call(pipe.as_handle().as_raw_handle(), &mut value) };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

/// The process identifier of the client of a pipe instance: `GetNamedPipeClientProcessId`.
///
/// # Example
///
/// ```no_run
/// use win_pipes_sys::get_named_pipe_client_process_id;
/// # fn server(pipe: std::os::windows::io::OwnedHandle) -> std::io::Result<()> {
/// let client = get_named_pipe_client_process_id(&pipe)?;
/// # let _ = client;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// The call's own error, which the page does not enumerate.
pub fn get_named_pipe_client_process_id(pipe: &impl AsHandle) -> io::Result<u32> {
    identifier(pipe, GetNamedPipeClientProcessId)
}

/// The session identifier of the client of a pipe instance: `GetNamedPipeClientSessionId`.
///
/// # Errors
///
/// The call's own error, which the page does not enumerate.
pub fn get_named_pipe_client_session_id(pipe: &impl AsHandle) -> io::Result<u32> {
    identifier(pipe, GetNamedPipeClientSessionId)
}

/// The process identifier of the server of a pipe instance: `GetNamedPipeServerProcessId`.
///
/// # Errors
///
/// The call's own error, which the page does not enumerate.
pub fn get_named_pipe_server_process_id(pipe: &impl AsHandle) -> io::Result<u32> {
    identifier(pipe, GetNamedPipeServerProcessId)
}

/// The session identifier of the server of a pipe instance: `GetNamedPipeServerSessionId`.
///
/// # Errors
///
/// The call's own error, which the page does not enumerate.
pub fn get_named_pipe_server_session_id(pipe: &impl AsHandle) -> io::Result<u32> {
    identifier(pipe, GetNamedPipeServerSessionId)
}

/// The name of the computer of the client of a pipe instance: `GetNamedPipeClientComputerNameW`.
///
/// `capacity_bytes` is `ClientComputerNameLength`, which the page gives as "the size of the
/// `ClientComputerName` buffer, in bytes", and no figure for how large a name can be: the caller says
/// how much room to give the call. A buffer is made of that many bytes, to a whole number of UTF-16
/// units, and the call is told the size of the buffer it is given. The result is the string up to its
/// first NUL; a unit that is not valid UTF-16 is kept. A capacity too small for the name is the call's
/// error.
///
/// # Example
///
/// ```no_run
/// use win_pipes_sys::get_named_pipe_client_computer_name;
/// # fn server(pipe: std::os::windows::io::OwnedHandle) -> std::io::Result<()> {
/// let computer = get_named_pipe_client_computer_name(&pipe, 1024)?;
/// # let _ = computer;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// The call's own error, which the page does not enumerate.
pub fn get_named_pipe_client_computer_name(
    pipe: &impl AsHandle,
    capacity_bytes: u32,
) -> io::Result<OsString> {
    let (mut buffer, buffer_bytes) = computer_name_buffer(capacity_bytes);

    // SAFETY: the handle is open for the borrow, and `buffer` is valid for `buffer_bytes` bytes, the
    // size the call is told, for the call.
    let ok = unsafe {
        GetNamedPipeClientComputerNameW(
            pipe.as_handle().as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer_bytes,
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(from_wide_until_nul(&buffer))
}

/// A zeroed buffer of whole UTF-16 units that fits in `capacity_bytes`, and its size in bytes, which is
/// what the call is told: never more than the buffer holds.
fn computer_name_buffer(capacity_bytes: u32) -> (Vec<u16>, u32) {
    let units = capacity_bytes / 2;
    (vec![0u16; units as usize], units * 2)
}
