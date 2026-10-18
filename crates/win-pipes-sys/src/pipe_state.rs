// Copyright (c) 2026 Mike Grier
//! Reading and changing what a pipe handle is and how it is in use: `GetNamedPipeInfo`,
//! `GetNamedPipeHandleStateW` and `SetNamedPipeHandleState`.
//!
//! Read 2026-10-18 from the three pages (`ms.date` 12/05/2018 for `GetNamedPipeInfo` and
//! `SetNamedPipeHandleState`, and 08/05/2022 for `GetNamedPipeHandleStateW`). Each call's conditions on
//! its arguments are left to the call, as in [`create_named_pipe`](crate::create_named_pipe): the
//! optional parameters the pages say "must be NULL" in some circumstances are requests the caller
//! chooses to make or not, so the caller who asks for what the call refuses gets the call's error.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::ptr;

use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::System::Pipes::{
    GetNamedPipeHandleStateW, GetNamedPipeInfo, PIPE_CLIENT_END, PIPE_SERVER_END, PIPE_TYPE_BYTE,
    PIPE_TYPE_MESSAGE, SetNamedPipeHandleState,
};

use crate::PipeMode;

#[cfg(test)]
mod tests;

/// The flags `GetNamedPipeInfo` reports in `lpFlags`: which end of the pipe the handle is, and the
/// pipe's type.
///
/// A newtype over the Win32 bits for the reason [`PipeMode`] is. Two of the four values the page
/// lists are zero ([`CLIENT_END`](Self::CLIENT_END) and [`TYPE_BYTE`](Self::TYPE_BYTE)), so
/// [`contains`](Self::contains) is true of them for every value; [`is_server_end`](Self::is_server_end)
/// and [`is_message_type`](Self::is_message_type) are the questions that can be answered.
///
/// # Example
///
/// ```
/// use win_pipes_sys::PipeInfoFlags;
///
/// let flags = PipeInfoFlags::SERVER_END | PipeInfoFlags::TYPE_MESSAGE;
/// assert!(flags.is_server_end());
/// assert!(flags.is_message_type());
/// assert!(!PipeInfoFlags::default().is_server_end());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PipeInfoFlags(u32);

impl PipeInfoFlags {
    /// `PIPE_CLIENT_END` (`0x0`): "The handle refers to the client end of a named pipe instance."
    pub const CLIENT_END: Self = Self(PIPE_CLIENT_END);

    /// `PIPE_SERVER_END` (`0x1`): "The handle refers to the server end of a named pipe instance."
    pub const SERVER_END: Self = Self(PIPE_SERVER_END);

    /// `PIPE_TYPE_BYTE` (`0x0`): "The named pipe is a byte pipe."
    pub const TYPE_BYTE: Self = Self(PIPE_TYPE_BYTE);

    /// `PIPE_TYPE_MESSAGE` (`0x4`): "The named pipe is a message pipe."
    pub const TYPE_MESSAGE: Self = Self(PIPE_TYPE_MESSAGE);

    /// The flags from their Win32 bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The flags' Win32 bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every bit of `other` is in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the handle is the server end: the page says that "if this value is not specified, the
    /// handle refers to the client end".
    #[must_use]
    pub const fn is_server_end(self) -> bool {
        self.contains(Self::SERVER_END)
    }

    /// Whether the pipe is a message pipe: the page says that "if this value is not specified, the
    /// pipe is a byte pipe".
    #[must_use]
    pub const fn is_message_type(self) -> bool {
        self.contains(Self::TYPE_MESSAGE)
    }
}

impl std::ops::BitOr for PipeInfoFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// What `GetNamedPipeInfo` reports of a pipe, field for field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipeInfo {
    /// `lpFlags`: the end the handle is and the pipe's type.
    pub flags: PipeInfoFlags,
    /// `lpOutBufferSize`: "the size of the buffer for outgoing data, in bytes. If the buffer size is
    /// zero, the buffer is allocated as needed."
    pub out_buffer_size: u32,
    /// `lpInBufferSize`: "the size of the buffer for incoming data, in bytes. If the buffer size is
    /// zero, the buffer is allocated as needed."
    pub in_buffer_size: u32,
    /// `lpMaxInstances`: "the maximum number of pipe instances that can be created", where
    /// [`NamedPipeOptions::UNLIMITED_INSTANCES`](crate::NamedPipeOptions::UNLIMITED_INSTANCES)
    /// means "limited only by the availability of system resources".
    pub max_instances: u32,
}

/// Reports what a pipe is: `GetNamedPipeInfo`.
///
/// All four of the call's optional outputs are asked for. `pipe` is "a handle to the named pipe
/// instance", which "must have `GENERIC_READ` access to the named pipe for a read-only or read/write
/// pipe, or it must have `GENERIC_WRITE` and `FILE_READ_ATTRIBUTES` access for a write-only pipe", and
/// "can also be a handle to an anonymous pipe, as returned by the `CreatePipe` function".
///
/// # Example
///
/// ```
/// use win_pipes_sys::{create_pipe, get_named_pipe_info};
///
/// let pipe = create_pipe(0, None)?;
/// let info = get_named_pipe_info(&pipe.read)?;
/// assert!(!info.flags.is_message_type());
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own error, which the page does not enumerate.
pub fn get_named_pipe_info(pipe: &impl AsHandle) -> io::Result<PipeInfo> {
    let mut flags = 0u32;
    let mut out_buffer_size = 0u32;
    let mut in_buffer_size = 0u32;
    let mut max_instances = 0u32;
    // SAFETY: the handle is open for the borrow and the four pointers are to locals that outlive
    // the call.
    let ok = unsafe {
        GetNamedPipeInfo(
            pipe.as_handle().as_raw_handle(),
            &mut flags,
            &mut out_buffer_size,
            &mut in_buffer_size,
            &mut max_instances,
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(PipeInfo {
        flags: PipeInfoFlags(flags),
        out_buffer_size,
        in_buffer_size,
        max_instances,
    })
}

/// Which of the values of `GetNamedPipeHandleStateW` to ask for. Each is an optional output of the
/// call, and the page makes some of them an error to ask for: the collection count and the timeout
/// "must be NULL if the specified pipe handle is to the server end of a named pipe or if client and
/// server processes are on the same computer", and the user name "must be NULL if the specified pipe
/// handle is to the client end". Nothing is asked for by default.
///
/// # Example
///
/// ```
/// use win_pipes_sys::PipeStateQuery;
///
/// let query = PipeStateQuery { state: true, ..PipeStateQuery::default() };
/// assert!(query.state && !query.current_instances);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PipeStateQuery {
    /// `lpState`.
    pub state: bool,
    /// `lpCurInstances`.
    pub current_instances: bool,
    /// `lpMaxCollectionCount`.
    pub max_collection_count: bool,
    /// `lpCollectDataTimeout`.
    pub collect_data_timeout: bool,
    /// `lpUserName`, with `nMaxUserNameSize` as the capacity in UTF-16 units, the page's "TCHARs":
    /// `None` does not ask for it.
    pub user_name_capacity: Option<u32>,
}

/// What `GetNamedPipeHandleStateW` reported, field for field: `Some` for each value that was asked
/// for, and `None` for each that was not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipeHandleState {
    /// `lpState`: of the two flags the page lists, [`PipeMode::NOWAIT`] is "the pipe handle is in
    /// nonblocking mode" and [`PipeMode::READMODE_MESSAGE`] is "the pipe handle is in message-read
    /// mode"; a flag that is not set means blocking mode and byte-read mode.
    pub state: Option<PipeMode>,
    /// `lpCurInstances`: "the number of current pipe instances".
    pub current_instances: Option<u32>,
    /// `lpMaxCollectionCount`: "the maximum number of bytes to be collected on the client's computer
    /// before transmission to the server".
    pub max_collection_count: Option<u32>,
    /// `lpCollectDataTimeout`: "the maximum time, in milliseconds, that can pass before a remote
    /// named pipe transfers information over the network".
    pub collect_data_timeout_ms: Option<u32>,
    /// `lpUserName`: "the user name string associated with the client application", which "the
    /// server can only retrieve ... if the client opened the pipe with `SECURITY_IMPERSONATION`
    /// access". The string up to its first NUL; a unit that is not valid UTF-16 is kept.
    pub user_name: Option<OsString>,
}

/// Reports how a pipe handle is in use: `GetNamedPipeHandleStateW`.
///
/// Only what `query` asks for is requested of the call, and only that is `Some` in the result. The
/// page says the call "returns successfully even if all of the pointers passed to it are NULL", which
/// is the default query. `pipe` has the access `GetNamedPipeInfo` needs, and can also be an anonymous
/// pipe's handle. The information "can vary during the lifetime of an instance of the named pipe".
///
/// # Example
///
/// ```
/// use win_pipes_sys::{PipeMode, PipeStateQuery, create_pipe, get_named_pipe_handle_state};
///
/// let pipe = create_pipe(0, None)?;
/// let query = PipeStateQuery { state: true, ..PipeStateQuery::default() };
/// let state = get_named_pipe_handle_state(&pipe.read, &query)?;
/// assert_eq!(state.state, Some(PipeMode::default()));
/// assert_eq!(state.current_instances, None);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own error. A name that does not fit in `user_name_capacity` is the call's.
pub fn get_named_pipe_handle_state(
    pipe: &impl AsHandle,
    query: &PipeStateQuery,
) -> io::Result<PipeHandleState> {
    let mut state = 0u32;
    let mut current_instances = 0u32;
    let mut max_collection_count = 0u32;
    let mut collect_data_timeout = 0u32;
    let mut user_name = vec![0u16; query.user_name_capacity.unwrap_or(0) as usize];

    let out = |wanted: bool, value: &mut u32| {
        if wanted {
            ptr::from_mut(value)
        } else {
            ptr::null_mut()
        }
    };
    let user_name_pointer = if query.user_name_capacity.is_some() {
        user_name.as_mut_ptr()
    } else {
        ptr::null_mut()
    };

    // SAFETY: the handle is open for the borrow; each output pointer is null or to a local that
    // outlives the call; and `user_name_pointer` is null, with a size the call ignores, or to
    // `user_name`, whose length is the size given.
    let ok = unsafe {
        GetNamedPipeHandleStateW(
            pipe.as_handle().as_raw_handle(),
            out(query.state, &mut state),
            out(query.current_instances, &mut current_instances),
            out(query.max_collection_count, &mut max_collection_count),
            out(query.collect_data_timeout, &mut collect_data_timeout),
            user_name_pointer,
            query.user_name_capacity.unwrap_or(0),
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }

    Ok(PipeHandleState {
        state: query.state.then(|| PipeMode::from_bits(state)),
        current_instances: query.current_instances.then_some(current_instances),
        max_collection_count: query.max_collection_count.then_some(max_collection_count),
        collect_data_timeout_ms: query.collect_data_timeout.then_some(collect_data_timeout),
        user_name: query.user_name_capacity.map(|_| {
            let end = user_name
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(user_name.len());
            OsString::from_wide(&user_name[..end])
        }),
    })
}

/// What to change with `SetNamedPipeHandleState`: each is an optional input of the call, and
/// nothing is changed by default.
///
/// # Example
///
/// ```
/// use win_pipes_sys::{PipeMode, PipeStateUpdate};
///
/// let update = PipeStateUpdate { mode: Some(PipeMode::NOWAIT), ..PipeStateUpdate::default() };
/// assert_eq!(update.max_collection_count, None);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PipeStateUpdate {
    /// `lpMode`: "the new pipe mode", "a combination of a read-mode flag and a wait-mode flag", of
    /// which [`PipeMode::READMODE_BYTE`], [`PipeMode::READMODE_MESSAGE`], [`PipeMode::WAIT`] and
    /// [`PipeMode::NOWAIT`] are the ones the page lists. The page says the call "fails if this flag
    /// is specified for a byte-type pipe" of the message read mode.
    pub mode: Option<PipeMode>,
    /// `lpMaxCollectionCount`: "the maximum number of bytes collected on the client computer before
    /// transmission to the server", which "must be NULL if the specified pipe handle is to the server
    /// end of a named pipe or if client and server processes are on the same machine".
    pub max_collection_count: Option<u32>,
    /// `lpCollectDataTimeout`: "the maximum time, in milliseconds, that can pass before a remote
    /// named pipe transfers information over the network", with the same condition.
    pub collect_data_timeout_ms: Option<u32>,
}

/// Sets the read mode and the blocking mode of a pipe handle, and for a client of a remote pipe its
/// local buffering: `SetNamedPipeHandleState`.
///
/// `pipe` "can be a handle to the server end of the pipe ... or to the client end", or to an anonymous
/// pipe, and "must have `GENERIC_WRITE` access to the named pipe for a write-only or read/write pipe,
/// or it must have `GENERIC_READ` and `FILE_WRITE_ATTRIBUTES` access for a read-only pipe". The page
/// calls nonblocking mode "supported for compatibility with Microsoft LAN Manager version 2.0" and
/// says it "should not be used to achieve asynchronous input and output".
///
/// # Example
///
/// ```
/// use win_pipes_sys::{
///     PipeMode, PipeStateQuery, PipeStateUpdate, create_pipe, get_named_pipe_handle_state,
///     set_named_pipe_handle_state,
/// };
///
/// let pipe = create_pipe(0, None)?;
/// let update = PipeStateUpdate { mode: Some(PipeMode::NOWAIT), ..PipeStateUpdate::default() };
/// set_named_pipe_handle_state(&pipe.read, &update)?;
///
/// let query = PipeStateQuery { state: true, ..PipeStateQuery::default() };
/// let state = get_named_pipe_handle_state(&pipe.read, &query)?;
/// assert_eq!(state.state, Some(PipeMode::NOWAIT));
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own error.
pub fn set_named_pipe_handle_state(
    pipe: &impl AsHandle,
    update: &PipeStateUpdate,
) -> io::Result<()> {
    let mode = update.mode.map(PipeMode::bits);
    let max_collection_count = update.max_collection_count;
    let collect_data_timeout = update.collect_data_timeout_ms;
    let optional = |value: &Option<u32>| value.as_ref().map_or(ptr::null(), ptr::from_ref);

    // SAFETY: the handle is open for the borrow, and each input pointer is null or to a local that
    // outlives the call.
    let ok = unsafe {
        SetNamedPipeHandleState(
            pipe.as_handle().as_raw_handle(),
            optional(&mode),
            optional(&max_collection_count),
            optional(&collect_data_timeout),
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
