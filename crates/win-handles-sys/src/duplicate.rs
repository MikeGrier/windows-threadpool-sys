// Copyright (c) 2026 Mike Grier
// Split from crates/win-sync-sys/src/duplicate.rs at a9a97bf6.
//! Duplicating a handle, with the access and the inheritance chosen: `DuplicateHandle`.

use std::io;
use std::ops::BitOr;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, FALSE, HANDLE, TRUE};
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

/// The access a duplicate carries: the source's own, or a mask chosen for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DuplicateAccess {
    /// Whatever access the source handle has, as `DUPLICATE_SAME_ACCESS` asks. Not a mask of
    /// nothing: the duplicate can do what the source can.
    Same,
    /// Exactly this mask, whatever the source holds.
    Chosen(Access),
}

/// Whether a child process can inherit a duplicate.
///
/// This is the duplicate's own attribute, set by the call and independent of the source's: an
/// inheritable source can be duplicated to a handle that is not, and the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Inheritance {
    /// Not inheritable: a process this one creates does not receive the handle.
    NotInheritable,
    /// Inheritable: a process this one creates receives the handle when it is created asking to
    /// inherit handles.
    Inheritable,
}

/// A second handle to the object `handle` refers to, in this process, with the access and
/// inheritance chosen.
///
/// With [`DuplicateAccess::Chosen`] the result can do no more than the mask allows, which is the
/// use: a handle with `SYNCHRONIZE` alone can wait on an object and change nothing about it. With
/// [`DuplicateAccess::Same`] it can do what the source can, so a caller that only needs another
/// handle to the object does not have to name the rights. The object lives until every handle to it
/// is closed, and closing the result does not touch the source.
///
/// **Windows does not confine the result to the source's access.** A duplicate may ask for rights
/// the source handle lacks and get them, so a holder of a narrow handle can make a wider one with
/// this very function. The narrowing is therefore a guard against a handle being used for what it
/// was not handed out for, not a boundary against code in the process that sets out to cross it --
/// no such boundary exists inside a process, and raw `DuplicateHandle` crosses it just as well.
///
/// A chosen access of nothing is accepted and gives a handle that can do nothing.
///
/// # Errors
///
/// The error from `DuplicateHandle`: access denied for a chosen mask Windows refuses, such as one
/// with every bit set, and otherwise only when the process cannot get another handle, which no test
/// reaches.
pub fn duplicate_handle(
    handle: impl AsHandle,
    access: DuplicateAccess,
    inheritance: Inheritance,
) -> io::Result<OwnedHandle> {
    let (desired_access, options) = match access {
        DuplicateAccess::Same => (0, DUPLICATE_SAME_ACCESS),
        DuplicateAccess::Chosen(mask) => (mask.bits(), 0),
    };
    let inherit = match inheritance {
        Inheritance::NotInheritable => FALSE,
        Inheritance::Inheritable => TRUE,
    };
    let mut duplicate: HANDLE = ptr::null_mut();
    // SAFETY: both process handles are the current-process pseudo-handle, the source is open for
    // the borrow, and `duplicate` is a valid out pointer.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_handle().as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            desired_access,
            inherit,
            options,
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `duplicate` is a new handle nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

/// A second handle to the object `handle` refers to, in this process, carrying exactly `access`.
///
/// This is [`duplicate_handle`] with [`DuplicateAccess::Chosen`] and [`Inheritance::NotInheritable`],
/// which is what its documentation says of the access, the narrowing and the errors. Not
/// inheritable by child processes.
///
/// # Errors
///
/// The errors of [`duplicate_handle`].
pub fn duplicate_handle_with_access(
    handle: impl AsHandle,
    access: Access,
) -> io::Result<OwnedHandle> {
    duplicate_handle(
        handle,
        DuplicateAccess::Chosen(access),
        Inheritance::NotInheritable,
    )
}

#[cfg(test)]
mod tests;
