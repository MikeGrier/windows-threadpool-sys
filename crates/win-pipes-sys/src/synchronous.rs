// Copyright (c) Mike Grier.
//! A pipe handle that was not opened for overlapped I/O, so that the calls that wait can take it
//! safely.
//!
//! `ConnectNamedPipe` and `TransactNamedPipe` with no `OVERLAPPED` are correct only on a handle that
//! was not opened with `FILE_FLAG_OVERLAPPED`: their pages say that on one that was "the function can
//! incorrectly report that the \[connect\] operation is complete", and a transaction that is reported complete
//! while its read is outstanding has let go of a buffer Windows still writes into. Whether a handle
//! was opened with the flag is decided when it is opened and cannot be changed, and no documented
//! call reports it, so it cannot be checked when the handle is used. [`SynchronousPipe`] carries it
//! instead: it is made only by opening a handle without the flag, or by a caller who states, in an
//! `unsafe` block, that their handle was opened so.

use std::ffi::OsStr;
use std::io;
use std::os::windows::io::{AsHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use win_authorization_sys::SecurityAttributes;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING};

use crate::name::wide_name;
use crate::{NamedPipeOptions, PipeOpenMode, create_named_pipe};

#[cfg(test)]
mod tests;

/// The arguments of the `CreateFileW` that opens a pipe's client end, after the name and the security
/// attributes. Nothing is defaulted: each field is the parameter it is named for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PipeClientOptions {
    /// `dwDesiredAccess`, as given. The `SetNamedPipeHandleState` page says a handle it changes must
    /// have `GENERIC_WRITE` or `FILE_WRITE_ATTRIBUTES` access.
    pub desired_access: u32,
    /// `dwShareMode`, as given.
    pub share_mode: u32,
    /// `dwFlagsAndAttributes`, as given, except that `FILE_FLAG_OVERLAPPED` is refused: a handle with
    /// it is not a [`SynchronousPipe`].
    pub flags_and_attributes: u32,
}

/// A pipe handle that was not opened with `FILE_FLAG_OVERLAPPED`, closed when it is dropped.
///
/// Made by [`create`](Self::create), which creates the server end of a named pipe instance, by
/// [`open_client`](Self::open_client), which opens a client end, each refusing the overlapped flag
/// before the call; or, for a handle opened some other way, by
/// [`from_owned_unchecked`](Self::from_owned_unchecked), whose caller states that it was opened
/// without the flag. [`connect_named_pipe`](crate::connect_named_pipe) and
/// [`transact_named_pipe`](crate::transact_named_pipe) take one and are safe because of it.
///
/// # Example
///
/// ```
/// use win_pipes_sys::{
///     NamedPipeOptions, PipeClientOptions, PipeMode, PipeOpenMode, SynchronousPipe,
///     connect_named_pipe, ConnectOutcome,
/// };
///
/// const GENERIC_READ: u32 = 0x8000_0000;
/// const GENERIC_WRITE: u32 = 0x4000_0000;
///
/// let name = format!(r"\\.\pipe\win-pipes-sys-synchronous-doc-{}", std::process::id());
/// let options = NamedPipeOptions {
///     open_mode: PipeOpenMode::ACCESS_DUPLEX,
///     pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
///     max_instances: 1,
///     out_buffer_size: 4096,
///     in_buffer_size: 4096,
///     default_timeout_ms: 0,
/// };
/// let server = SynchronousPipe::create(&name, &options, None)?;
/// let client_options = PipeClientOptions {
///     desired_access: GENERIC_READ | GENERIC_WRITE,
///     ..PipeClientOptions::default()
/// };
/// let _client = SynchronousPipe::open_client(&name, &client_options, None)?;
///
/// assert_eq!(connect_named_pipe(&server)?, ConnectOutcome::AlreadyConnected);
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct SynchronousPipe {
    handle: OwnedHandle,
}

/// The crate's own refusal of the overlapped flag: not the call's, so it carries no OS code.
fn overlapped_refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "a SynchronousPipe is not opened with FILE_FLAG_OVERLAPPED",
    )
}

impl SynchronousPipe {
    /// Creates an instance of the named pipe `name` and returns its server end, as
    /// [`create_named_pipe`] does, for options without [`PipeOpenMode::OVERLAPPED`].
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`], with no OS code, for options whose open mode has
    /// [`PipeOpenMode::OVERLAPPED`], without calling `CreateNamedPipeW`: this crate's own narrowing,
    /// since such a handle is not one of these. Otherwise the errors of [`create_named_pipe`].
    pub fn create(
        name: impl AsRef<OsStr>,
        options: &NamedPipeOptions,
        security: Option<&SecurityAttributes>,
    ) -> io::Result<Self> {
        if options.open_mode.contains(PipeOpenMode::OVERLAPPED) {
            return Err(overlapped_refused());
        }
        create_named_pipe(name, options, security).map(|handle| Self { handle })
    }

    /// Opens a client end of the named pipe `name`: `CreateFileW` with `OPEN_EXISTING`, since a client
    /// opens an instance that exists, and no template.
    ///
    /// `security` is `lpSecurityAttributes`: `None` passes none, and `Some` decides whether the handle
    /// can be inherited.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`], with no OS code and without calling `CreateFileW`, for a name
    /// with an interior NUL, and for flags that include `FILE_FLAG_OVERLAPPED` (this crate's own
    /// narrowing). Otherwise the call's own error, such as `ERROR_FILE_NOT_FOUND` for a name with no
    /// instance and `ERROR_PIPE_BUSY` when every instance is connected.
    pub fn open_client(
        name: impl AsRef<OsStr>,
        options: &PipeClientOptions,
        security: Option<&SecurityAttributes>,
    ) -> io::Result<Self> {
        if options.flags_and_attributes & FILE_FLAG_OVERLAPPED != 0 {
            return Err(overlapped_refused());
        }
        let wide = wide_name(name.as_ref())?;
        let raw_security = security.map(SecurityAttributes::to_raw);
        let security_pointer = raw_security.as_ref().map_or(ptr::null(), ptr::from_ref);

        // SAFETY: `wide` is NUL-terminated and outlives the call, `security_pointer` is null or points
        // at `raw_security`, whose descriptor pointer is into `security`, both of which outlive the
        // call, and no template is given.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                options.desired_access,
                options.share_mode,
                security_pointer,
                OPEN_EXISTING,
                options.flags_and_attributes,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful call returns a handle that nothing else owns.
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        })
    }

    /// Takes `handle` as a pipe that was not opened for overlapped I/O.
    ///
    /// # Safety
    ///
    /// `handle` must not have been opened with `FILE_FLAG_OVERLAPPED`: by `CreateNamedPipeW` without
    /// it in the open mode, by `CreateFileW` without it in the flags, or by `CreatePipe` (the
    /// "Anonymous Pipe Operations" page: "Asynchronous (overlapped) read and write operations are not
    /// supported by anonymous pipes"); or be a duplicate of such a handle.
    /// The calls that take a `SynchronousPipe` pass no `OVERLAPPED`, which is correct only then.
    #[must_use]
    pub unsafe fn from_owned_unchecked(handle: OwnedHandle) -> Self {
        Self { handle }
    }

    /// A second handle to the same pipe end, which shares how it was opened and so is one of these
    /// too: `DuplicateHandle` through [`OwnedHandle::try_clone`].
    ///
    /// # Errors
    ///
    /// The duplication's own error.
    pub fn try_clone(&self) -> io::Result<Self> {
        self.handle.try_clone().map(|handle| Self { handle })
    }

    /// The handle, for a caller that no longer needs this type's knowledge of it.
    #[must_use]
    pub fn into_owned(self) -> OwnedHandle {
        self.handle
    }
}

impl AsHandle for SynchronousPipe {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }
}

impl From<SynchronousPipe> for OwnedHandle {
    fn from(pipe: SynchronousPipe) -> Self {
        pipe.handle
    }
}
