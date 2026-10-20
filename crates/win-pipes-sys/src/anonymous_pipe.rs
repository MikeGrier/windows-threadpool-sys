// Copyright (c) 2026 Mike Grier
//! Creating an anonymous pipe: `CreatePipe`.
//!
//! Read 2026-10-18 from the call's page (`ms.date` 12/05/2018).

use std::io;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr;

use win_authorization_sys::SecurityAttributes;
use windows_sys::Win32::Foundation::{FALSE, HANDLE};
use windows_sys::Win32::System::Pipes::CreatePipe;

#[cfg(test)]
mod tests;

/// The two ends of an anonymous pipe, each closed when it is dropped.
///
/// The ends are named fields and not a tuple so that the read end cannot be taken for the write end
/// by position. The page says the read handle is the one to give to `ReadFile` and the write handle
/// the one to give to `WriteFile`; which direction a handle allows is the call's, and is not checked
/// here.
#[derive(Debug)]
pub struct AnonymousPipe {
    /// The read end, `hReadPipe`.
    pub read: OwnedHandle,
    /// The write end, `hWritePipe`.
    pub write: OwnedHandle,
}

/// Creates an anonymous pipe and returns its two ends: `CreatePipe`.
///
/// `size` is `nSize`, "the size of the buffer for the pipe, in bytes", which "is only a suggestion;
/// the system uses the value to calculate an appropriate buffering mechanism", and for zero "the
/// system uses the default buffer size". `security` is the attributes: `None` passes none, so "the
/// handle cannot be inherited" and "the pipe gets a default security descriptor", and `Some` gives
/// the pipe its descriptor and "determines whether the returned handle can be inherited by child
/// processes". The one flag applies to both ends: the call has one attributes parameter and no way to
/// give the two ends different ones.
///
/// The page says that "anonymous pipes are implemented using a named pipe with a unique name", so a
/// handle to one can "often" be given to a function that wants a named pipe's. A write that finds the
/// pipe full "does not return until another process or thread uses `ReadFile` to make more buffer
/// space available".
///
/// # Example
///
/// ```
/// use std::fs::File;
/// use std::io::{Read, Write};
///
/// use win_pipes_sys::create_pipe;
///
/// let pipe = create_pipe(0, None)?;
/// let mut write = File::from(pipe.write);
/// let mut read = File::from(pipe.read);
///
/// write.write_all(b"ping")?;
/// let mut received = [0u8; 4];
/// read.read_exact(&mut received)?;
/// assert_eq!(&received, b"ping");
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own error. The page says that after a failure "the contents of the output parameters
/// are indeterminate", so no handle is taken from them.
pub fn create_pipe(size: u32, security: Option<&SecurityAttributes>) -> io::Result<AnonymousPipe> {
    let raw_security = security.map(SecurityAttributes::to_raw);
    let security_pointer = raw_security.as_ref().map_or(ptr::null(), ptr::from_ref);

    let mut read: HANDLE = ptr::null_mut();
    let mut write: HANDLE = ptr::null_mut();
    // SAFETY: both output pointers are to locals that outlive the call, and `security_pointer` is
    // null or points at `raw_security`, whose descriptor pointer is into `security`, both of which
    // outlive the call.
    let created = unsafe { CreatePipe(&mut read, &mut write, security_pointer, size) };
    if created == FALSE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful call returns two handles that nothing else owns.
    Ok(unsafe {
        AnonymousPipe {
            read: OwnedHandle::from_raw_handle(read),
            write: OwnedHandle::from_raw_handle(write),
        }
    })
}
