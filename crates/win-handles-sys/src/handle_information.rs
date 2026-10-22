// Copyright (c) 2026 Mike Grier
//! What a handle can be asked about itself, and told: `GetHandleInformation` and
//! `SetHandleInformation`.

use std::io;
use std::ops::BitOr;
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::{
    FALSE, GetHandleInformation, HANDLE_FLAG_INHERIT, HANDLE_FLAG_PROTECT_FROM_CLOSE,
    SetHandleInformation,
};

/// The flags a handle carries: whether a child process created with handle inheritance gets a copy of
/// it, and whether closing it is refused.
///
/// A newtype over the Win32 bits, combined with `|`, because Windows defines the two the page
/// documents and this crate does not narrow what the call reports: a bit it has never heard of is
/// kept by [`from_bits`](Self::from_bits) and [`bits`](Self::bits) and is not an error.
///
/// # Example
///
/// ```
/// use win_handles_sys::HandleFlags;
///
/// let both = HandleFlags::INHERIT | HandleFlags::PROTECT_FROM_CLOSE;
/// assert!(both.contains(HandleFlags::INHERIT));
/// assert!(both.contains(HandleFlags::PROTECT_FROM_CLOSE));
/// assert!(!HandleFlags::INHERIT.contains(HandleFlags::PROTECT_FROM_CLOSE));
/// assert_eq!(HandleFlags::default(), HandleFlags::NONE);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct HandleFlags(u32);

impl HandleFlags {
    /// Neither flag: the handle is not inherited and may be closed.
    pub const NONE: Self = Self(0);

    /// `HANDLE_FLAG_INHERIT`: a child process created with handle inheritance gets a copy of the handle.
    pub const INHERIT: Self = Self(HANDLE_FLAG_INHERIT);

    /// `HANDLE_FLAG_PROTECT_FROM_CLOSE`: closing the handle is refused.
    pub const PROTECT_FROM_CLOSE: Self = Self(HANDLE_FLAG_PROTECT_FROM_CLOSE);

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

    /// Whether every flag in `other` is in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for HandleFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// The flags `handle` carries: `GetHandleInformation`.
///
/// # Example
///
/// A handle `std` opens is not inheritable:
///
/// ```
/// use std::fs::File;
///
/// use win_handles_sys::{HandleFlags, get_handle_information};
///
/// let path = std::env::temp_dir().join(format!("wh-info-{}.tmp", std::process::id()));
/// let file = File::create(&path)?;
///
/// assert!(!get_handle_information(&file)?.contains(HandleFlags::INHERIT));
/// # drop(file);
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The error from `GetHandleInformation`. A handle that is open for the borrow, which `AsHandle`
/// guarantees, is not refused, so no test reaches it.
pub fn get_handle_information(handle: impl AsHandle) -> io::Result<HandleFlags> {
    let mut flags = 0u32;
    // SAFETY: the handle is open for the borrow and `flags` is a valid out pointer.
    let ok = unsafe { GetHandleInformation(handle.as_handle().as_raw_handle(), &mut flags) };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(HandleFlags(flags))
}

/// Changes the flags `handle` carries: `SetHandleInformation`.
///
/// `mask` names the flags to change, and `flags` gives each of them its new value: a flag in `mask` is set if it is
/// in `flags` and cleared if it is not. The page says "To set or clear the associated bit flag in dwFlags, you must
/// set a change mask bit flag in dwMask", so a flag in `flags` that is not in `mask` is left as it was, and so is
/// every flag the handle carries that is not in `mask`. The flags are the handle's and not its object's: a duplicate
/// of it has its own.
///
/// Nothing is refused before the call: the page calls no value of either argument invalid, and a bit it does not
/// name is the call's to judge ([`HandleFlags`]).
///
/// A handle given [`PROTECT_FROM_CLOSE`](HandleFlags::PROTECT_FROM_CLOSE) is not closed by `CloseHandle`, which is
/// what dropping an `OwnedHandle` calls, so one dropped while protected stays open for the life of the process.
/// Clear the flag first.
///
/// # Example
///
/// ```
/// use std::fs::File;
///
/// use win_handles_sys::{HandleFlags, get_handle_information, set_handle_information};
///
/// let path = std::env::temp_dir().join(format!("wh-set-{}.tmp", std::process::id()));
/// let file = File::create(&path)?;
///
/// // Make the handle inheritable, and leave every other flag as it was.
/// set_handle_information(&file, HandleFlags::INHERIT, HandleFlags::INHERIT)?;
/// assert!(get_handle_information(&file)?.contains(HandleFlags::INHERIT));
///
/// // A flag in `flags` but not in the mask is not changed.
/// set_handle_information(&file, HandleFlags::NONE, HandleFlags::PROTECT_FROM_CLOSE)?;
/// assert!(!get_handle_information(&file)?.contains(HandleFlags::PROTECT_FROM_CLOSE));
///
/// // A flag in the mask and not in `flags` is cleared.
/// set_handle_information(&file, HandleFlags::INHERIT, HandleFlags::NONE)?;
/// assert_eq!(get_handle_information(&file)?, HandleFlags::NONE);
/// # drop(file);
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The error from `SetHandleInformation`, which the page does not list. The current process's pseudo-handle, which
/// `std` allows a `BorrowedHandle` to hold, was refused with `ERROR_ACCESS_DENIED` where it was measured
/// (WH-D-3).
pub fn set_handle_information(
    handle: impl AsHandle,
    mask: HandleFlags,
    flags: HandleFlags,
) -> io::Result<()> {
    // SAFETY: the handle is open for the borrow.
    let ok = unsafe { SetHandleInformation(handle.as_handle().as_raw_handle(), mask.0, flags.0) };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
