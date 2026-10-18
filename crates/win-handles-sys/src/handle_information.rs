// Copyright (c) 2026 Mike Grier
//! What a handle can be asked about itself: `GetHandleInformation`.

use std::io;
use std::ops::BitOr;
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::{
    FALSE, GetHandleInformation, HANDLE_FLAG_INHERIT, HANDLE_FLAG_PROTECT_FROM_CLOSE,
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

#[cfg(test)]
mod tests;
