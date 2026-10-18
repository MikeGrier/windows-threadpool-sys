// Copyright (c) 2026 Mike Grier
// Split from crates/win-sync-sys/src/duplicate.rs at a9a97bf6.
//! Duplicating a handle with a chosen access: `DuplicateHandle`.

use std::io;
use std::ops::BitOr;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{DuplicateHandle, FALSE, HANDLE};
use windows_sys::Win32::System::Threading::{
    EVENT_MODIFY_STATE, GetCurrentProcess, SYNCHRONIZATION_SYNCHRONIZE,
};

/// An access mask for a duplicated handle: what the holder of the handle may do with the object.
///
/// A mask is a set of rights, combined with `|`. The rights an object type defines are named by
/// Win32 (`EVENT_MODIFY_STATE` for an event, `SEMAPHORE_MODIFY_STATE` for a semaphore, and so on),
/// and the two the code this was moved from needed are constants here, `SYNCHRONIZE`, which every
/// object type defines, and `EVENT_MODIFY_STATE`, an event's; any other is
/// [`from_bits`](Self::from_bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Access(u32);

impl Access {
    /// `SYNCHRONIZE`: the right to wait on the object, and nothing else. A handle with only this
    /// can be waited on and cannot change the object.
    pub const SYNCHRONIZE: Self = Self(SYNCHRONIZATION_SYNCHRONIZE);

    /// `EVENT_MODIFY_STATE`: the right to set and reset an event.
    pub const EVENT_MODIFY_STATE: Self = Self(EVENT_MODIFY_STATE);

    /// A mask from its Win32 bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The mask's Win32 bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl BitOr for Access {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// A second handle to the object `handle` refers to, in this process, carrying exactly `access`.
///
/// The result can do no more than `access` allows, which is the use: a handle with `SYNCHRONIZE`
/// alone can wait on an object and change nothing about it. The object lives until every handle to
/// it is closed, and closing the result does not touch the source.
///
/// **Windows does not confine the result to the source's access.** A duplicate may ask for rights
/// the source handle lacks and get them, so a holder of a narrow handle can make a wider one with
/// this very function. The narrowing is therefore a guard against a handle being used for what it
/// was not handed out for, not a boundary against code in the process that sets out to cross it --
/// no such boundary exists inside a process, and raw `DuplicateHandle` crosses it just as well.
///
/// Not inheritable by child processes. An access of nothing is accepted and gives a handle that
/// can do nothing.
///
/// # Errors
///
/// The error from `DuplicateHandle`: access denied for a mask Windows refuses, such as one with
/// every bit set, and otherwise only when the process cannot get another handle, which no test
/// reaches.
pub fn duplicate_handle_with_access(
    handle: impl AsHandle,
    access: Access,
) -> io::Result<OwnedHandle> {
    let mut duplicate: HANDLE = ptr::null_mut();
    // SAFETY: both process handles are the current-process pseudo-handle, the source is open for
    // the borrow, and `duplicate` is a valid out pointer.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_handle().as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            access.bits(),
            FALSE,
            0,
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `duplicate` is a new handle nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

#[cfg(test)]
mod tests;
