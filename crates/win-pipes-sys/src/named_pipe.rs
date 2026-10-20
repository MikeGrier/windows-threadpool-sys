// Copyright (c) 2026 Mike Grier
//! Creating an instance of a named pipe: `CreateNamedPipeW`.
//!
//! Read 2026-10-18 from the call's page (`ms.date` 08/03/2022). The page's conditions on the
//! arguments are left to the call: this crate passes every argument as given and the call refuses
//! what it refuses, which is why [`PipeOpenMode`] and [`PipeMode`] are bit sets and not types that
//! make a combination unrepresentable. The one argument the crate cannot pass as given is a name
//! with an interior NUL, which a C string cannot hold.

use std::ffi::OsStr;
use std::io;
use std::ops::BitOr;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr;

use crate::name::wide_name;
use win_authorization_sys::SecurityAttributes;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAG_WRITE_THROUGH,
    PIPE_ACCESS_DUPLEX, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_ACCEPT_REMOTE_CLIENTS, PIPE_NOWAIT, PIPE_READMODE_BYTE,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_TYPE_MESSAGE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::SystemServices::ACCESS_SYSTEM_SECURITY;

#[cfg(test)]
mod tests;

/// The open mode of a pipe instance, `dwOpenMode`: the direction of the pipe, the write-through and
/// overlapped flags, and the security access the caller is given.
///
/// A newtype over the Win32 bits, combined with `|`, as `win_handles_sys::HandleFlags` is: the call
/// "fails if `dwOpenMode` specifies anything other than 0 or the flags listed", and this crate does
/// not decide that for it, so a bit it has no name for is held by [`from_bits`](Self::from_bits).
/// The page requires exactly one of the three direction modes and that every instance of a pipe
/// specify the same one; neither is checked here.
///
/// [`FIRST_PIPE_INSTANCE`](Self::FIRST_PIPE_INSTANCE) and [`WRITE_OWNER`](Self::WRITE_OWNER) are
/// listed on the page with the same value, `0x00080000`, so they are one bit with two names.
///
/// # Example
///
/// ```
/// use win_pipes_sys::PipeOpenMode;
///
/// let mode = PipeOpenMode::ACCESS_DUPLEX | PipeOpenMode::OVERLAPPED;
/// assert!(mode.contains(PipeOpenMode::OVERLAPPED));
/// assert!(!mode.contains(PipeOpenMode::WRITE_THROUGH));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PipeOpenMode(u32);

impl PipeOpenMode {
    /// `PIPE_ACCESS_INBOUND` (`0x1`): "The flow of data in the pipe goes from client to server only."
    pub const ACCESS_INBOUND: Self = Self(PIPE_ACCESS_INBOUND);

    /// `PIPE_ACCESS_OUTBOUND` (`0x2`): "The flow of data in the pipe goes from server to client only."
    pub const ACCESS_OUTBOUND: Self = Self(PIPE_ACCESS_OUTBOUND);

    /// `PIPE_ACCESS_DUPLEX` (`0x3`): "The pipe is bi-directional; both server and client processes can
    /// read from and write to the pipe."
    pub const ACCESS_DUPLEX: Self = Self(PIPE_ACCESS_DUPLEX);

    /// `FILE_FLAG_FIRST_PIPE_INSTANCE`: "If you attempt to create multiple instances of a pipe with
    /// this flag, creation of the first instance succeeds, but creation of the next instance fails
    /// with `ERROR_ACCESS_DENIED`."
    pub const FIRST_PIPE_INSTANCE: Self = Self(FILE_FLAG_FIRST_PIPE_INSTANCE);

    /// `FILE_FLAG_WRITE_THROUGH`: write-through mode, which "affects only write operations on
    /// byte-type pipes and, then, only when the client and server processes are on different
    /// computers".
    pub const WRITE_THROUGH: Self = Self(FILE_FLAG_WRITE_THROUGH);

    /// `FILE_FLAG_OVERLAPPED`: overlapped mode, in which read, write and connect operations that may
    /// take a significant time "can return immediately".
    pub const OVERLAPPED: Self = Self(FILE_FLAG_OVERLAPPED);

    /// `WRITE_DAC`: "The caller will have write access to the named pipe's discretionary access
    /// control list (ACL)."
    pub const WRITE_DAC: Self = Self(WRITE_DAC);

    /// `WRITE_OWNER`: "The caller will have write access to the named pipe's owner." The same bit as
    /// [`FIRST_PIPE_INSTANCE`](Self::FIRST_PIPE_INSTANCE), by the page's own values.
    pub const WRITE_OWNER: Self = Self(WRITE_OWNER);

    /// `ACCESS_SYSTEM_SECURITY`: "The caller will have write access to the named pipe's SACL."
    pub const ACCESS_SYSTEM_SECURITY: Self = Self(ACCESS_SYSTEM_SECURITY);

    /// The mode from its Win32 bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The mode's Win32 bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every bit of `other` is in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for PipeOpenMode {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// The pipe mode of a pipe instance, `dwPipeMode`: byte or message type, byte or message reading,
/// blocking or not, and whether remote clients are accepted.
///
/// A newtype over the Win32 bits, combined with `|`, for the reason [`PipeOpenMode`] is. Four of the
/// modes the page lists have the value zero ([`TYPE_BYTE`](Self::TYPE_BYTE),
/// [`READMODE_BYTE`](Self::READMODE_BYTE), [`WAIT`](Self::WAIT) and
/// [`ACCEPT_REMOTE_CLIENTS`](Self::ACCEPT_REMOTE_CLIENTS)): they are named because the page names
/// them, and [`contains`](Self::contains) is true of them for every mode, so it cannot say one was
/// chosen. The page requires that "this mode cannot be used with `PIPE_READMODE_MESSAGE`" of a byte
/// type, and that every instance of a pipe specify the same type; neither is checked here.
///
/// # Example
///
/// ```
/// use win_pipes_sys::PipeMode;
///
/// let mode = PipeMode::TYPE_MESSAGE | PipeMode::READMODE_MESSAGE | PipeMode::REJECT_REMOTE_CLIENTS;
/// assert!(mode.contains(PipeMode::READMODE_MESSAGE));
/// assert!(!mode.contains(PipeMode::NOWAIT));
/// assert_eq!(PipeMode::default(), PipeMode::TYPE_BYTE | PipeMode::WAIT);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PipeMode(u32);

impl PipeMode {
    /// `PIPE_TYPE_BYTE` (`0x0`): "Data is written to the pipe as a stream of bytes."
    pub const TYPE_BYTE: Self = Self(PIPE_TYPE_BYTE);

    /// `PIPE_TYPE_MESSAGE` (`0x4`): "Data is written to the pipe as a stream of messages."
    pub const TYPE_MESSAGE: Self = Self(PIPE_TYPE_MESSAGE);

    /// `PIPE_READMODE_BYTE` (`0x0`): "Data is read from the pipe as a stream of bytes."
    pub const READMODE_BYTE: Self = Self(PIPE_READMODE_BYTE);

    /// `PIPE_READMODE_MESSAGE` (`0x2`): "Data is read from the pipe as a stream of messages", which
    /// the page says "can be only used if `PIPE_TYPE_MESSAGE` is also specified".
    pub const READMODE_MESSAGE: Self = Self(PIPE_READMODE_MESSAGE);

    /// `PIPE_WAIT` (`0x0`): "Blocking mode is enabled."
    pub const WAIT: Self = Self(PIPE_WAIT);

    /// `PIPE_NOWAIT` (`0x1`): "Nonblocking mode is enabled", which the page says is "supported for
    /// compatibility with Microsoft LAN Manager version 2.0 and should not be used to achieve
    /// asynchronous I/O with named pipes".
    pub const NOWAIT: Self = Self(PIPE_NOWAIT);

    /// `PIPE_ACCEPT_REMOTE_CLIENTS` (`0x0`): "Connections from remote clients can be accepted and
    /// checked against the security descriptor for the pipe."
    pub const ACCEPT_REMOTE_CLIENTS: Self = Self(PIPE_ACCEPT_REMOTE_CLIENTS);

    /// `PIPE_REJECT_REMOTE_CLIENTS` (`0x8`): "Connections from remote clients are automatically
    /// rejected."
    pub const REJECT_REMOTE_CLIENTS: Self = Self(PIPE_REJECT_REMOTE_CLIENTS);

    /// The mode from its Win32 bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The mode's Win32 bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every bit of `other` is in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for PipeMode {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// The arguments of `CreateNamedPipeW` after the name and the security attributes, as the call takes
/// them. Nothing is defaulted: each field is the parameter it is named for.
///
/// # Example
///
/// ```
/// use win_pipes_sys::{NamedPipeOptions, PipeMode, PipeOpenMode};
///
/// let options = NamedPipeOptions {
///     open_mode: PipeOpenMode::ACCESS_DUPLEX,
///     pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
///     max_instances: 1,
///     out_buffer_size: 4096,
///     in_buffer_size: 4096,
///     default_timeout_ms: 0,
/// };
/// assert_eq!(options.max_instances, 1);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NamedPipeOptions {
    /// `dwOpenMode`.
    pub open_mode: PipeOpenMode,
    /// `dwPipeMode`.
    pub pipe_mode: PipeMode,
    /// `nMaxInstances`: "Acceptable values are in the range 1 through `PIPE_UNLIMITED_INSTANCES`
    /// (255)". A larger value is the call's refusal with `ERROR_INVALID_PARAMETER`, per the page.
    pub max_instances: u32,
    /// `nOutBufferSize`: the bytes to reserve for the output buffer, which the page calls advisory.
    pub out_buffer_size: u32,
    /// `nInBufferSize`: the bytes to reserve for the input buffer, which the page calls advisory.
    pub in_buffer_size: u32,
    /// `nDefaultTimeOut`: the default time-out in milliseconds for `WaitNamedPipe` when it asks for
    /// the default; the page says a zero "will result in a default time-out of 50 milliseconds".
    pub default_timeout_ms: u32,
}

impl NamedPipeOptions {
    /// `PIPE_UNLIMITED_INSTANCES` (255): the instance count for which "the number of pipe instances
    /// that can be created is limited only by the availability of system resources".
    pub const UNLIMITED_INSTANCES: u32 = PIPE_UNLIMITED_INSTANCES;
}

/// Creates an instance of the named pipe `name` and returns the server end of it: `CreateNamedPipeW`.
///
/// `name` is the pipe's name, which the page says "must have the following form:
/// `\\.\pipe\pipename`", can be "up to 256 characters long", and is not case sensitive; none of that
/// is checked here. `security` is the attributes for the new pipe: `None` passes none, so "the named
/// pipe gets a default security descriptor and the handle cannot be inherited", and `Some` gives the
/// pipe that descriptor and "determines whether child processes can inherit the returned handle".
///
/// The returned handle is closed when it is dropped. An instance "is always deleted when the last
/// handle to the instance of the named pipe is closed".
///
/// # Example
///
/// ```
/// use std::fs::OpenOptions;
/// use std::io::{Read, Write};
///
/// use win_pipes_sys::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};
///
/// let name = format!(r"\\.\pipe\win-pipes-sys-doc-{}", std::process::id());
/// let options = NamedPipeOptions {
///     open_mode: PipeOpenMode::ACCESS_DUPLEX,
///     pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
///     max_instances: 1,
///     out_buffer_size: 4096,
///     in_buffer_size: 4096,
///     default_timeout_ms: 0,
/// };
///
/// let mut server = std::fs::File::from(create_named_pipe(&name, &options, None)?);
/// let mut client = OpenOptions::new().read(true).write(true).open(&name)?;
///
/// client.write_all(b"ping")?;
/// let mut received = [0u8; 4];
/// server.read_exact(&mut received)?;
/// assert_eq!(&received, b"ping");
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a name with an interior NUL, which a C string cannot hold,
/// without calling `CreateNamedPipeW`. Otherwise the call's own error, such as `ERROR_ACCESS_DENIED`
/// for a second instance of a pipe made with [`PipeOpenMode::FIRST_PIPE_INSTANCE`], and
/// `ERROR_INVALID_PARAMETER` for more than [`NamedPipeOptions::UNLIMITED_INSTANCES`] instances, both
/// as the page says.
pub fn create_named_pipe(
    name: impl AsRef<OsStr>,
    options: &NamedPipeOptions,
    security: Option<&SecurityAttributes>,
) -> io::Result<OwnedHandle> {
    let wide = wide_name(name.as_ref())?;

    let raw_security = security.map(SecurityAttributes::to_raw);
    let security_pointer = raw_security.as_ref().map_or(ptr::null(), ptr::from_ref);

    // SAFETY: `wide` is NUL-terminated and outlives the call, and `security_pointer` is null or
    // points at `raw_security`, whose descriptor pointer is into `security`, both of which outlive
    // the call.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            options.open_mode.0,
            options.pipe_mode.0,
            options.max_instances,
            options.out_buffer_size,
            options.in_buffer_size,
            options.default_timeout_ms,
            security_pointer,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful call returns a handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
