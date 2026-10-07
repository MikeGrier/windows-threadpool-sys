// Copyright (c) 2026 Mike Grier
//! A shared owner of a Windows handle.
//!
//! std already covers two of the three ways to hold a handle: [`OwnedHandle`] owns one alone, and
//! [`BorrowedHandle`] lends one. This crate is the third, **shared ownership**: [`SharedHandle`] is
//! an [`OwnedHandle`] behind a reference count, so several holders can keep one handle open and it
//! closes when the last of them drops. Without it, every crate that needs the shared form wraps
//! `Arc<OwnedHandle>` under a name of its own.
//!
//! # What it gives
//!
//! - **Many holders.** Cloning is a reference-count increment, never a new handle. The handle
//!   closes when the last clone drops.
//! - **Lending.** [`AsHandle`] and [`AsRawHandle`], so a holder can pass the handle to a Windows call
//!   without taking ownership of it.
//! - **Conversions.** In from an [`OwnedHandle`] or a [`File`]; back out to an [`OwnedHandle`]
//!   through [`SharedHandle::try_into_owned`], for the last holder.
//! - **A same-handle test.** [`SharedHandle::same_handle`] says whether two values share one handle.
//!
//! # What it does not do
//!
//! - **No I/O.** It carries a handle; reading, writing and flushing are whoever uses it.
//! - **No claim about how the handle was opened.** Overlapped or not, buffered or not: nothing
//!   here checks or promises it.
//! - **Only the handles [`OwnedHandle`] is for** -- those closed with `CloseHandle`. Handles with
//!   another close function (an I/O ring, an enumeration, a thread-pool object) keep their own
//!   owners.
//!
//! There is no `unsafe` here and no native API surface: the crate is std's [`OwnedHandle`] and
//! [`Arc`] and nothing more.
//!
//! # Example
//!
//! ```
//! use std::fs::File;
//! use std::os::windows::io::{AsHandle, AsRawHandle};
//!
//! use win_shared_os_owned_handle::SharedHandle;
//!
//! let path = std::env::temp_dir().join(format!("wsooh-doc-{}.tmp", std::process::id()));
//! let first = SharedHandle::from(File::create(&path)?);
//!
//! // A clone shares the handle; it does not open or duplicate anything.
//! let second = first.clone();
//! assert!(first.same_handle(&second));
//! assert_eq!(second.as_handle().as_raw_handle(), first.as_raw_handle());
//!
//! // Dropping one holder leaves the handle open for the other.
//! drop(first);
//!
//! // The last holder can take the handle back as an `OwnedHandle`.
//! let owned = second.try_into_owned().expect("the only holder left");
//! drop(owned); // closes it
//! # std::fs::remove_file(&path)?;
//! # Ok::<(), std::io::Error>(())
//! ```

#![cfg(windows)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fs::File;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle, RawHandle};
use std::sync::Arc;

/// A shared owner of a Windows handle: an [`OwnedHandle`] behind a reference count.
///
/// Cloning is cheap and shares the one handle; the handle closes when the last clone drops. See
/// the [crate documentation](crate) for what it does and does not do.
#[derive(Clone, Debug)]
pub struct SharedHandle(Arc<OwnedHandle>);

impl SharedHandle {
    /// Take ownership of `handle`, as the first of its holders.
    #[must_use]
    pub fn new(handle: OwnedHandle) -> Self {
        Self(Arc::new(handle))
    }

    /// Whether `self` and `other` share one handle -- that is, whether one is a clone of the
    /// other, directly or not.
    ///
    /// Two handles opened separately on the same file are *not* the same handle, even though they
    /// name the same file.
    #[must_use]
    pub fn same_handle(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The handle, as an [`OwnedHandle`], if `self` is its only holder.
    ///
    /// # Errors
    ///
    /// If any clone still holds the handle, returns `self` unchanged, so nothing is lost and the
    /// caller can try again once the others have dropped.
    pub fn try_into_owned(self) -> Result<OwnedHandle, Self> {
        Arc::try_unwrap(self.0).map_err(Self)
    }
}

impl From<OwnedHandle> for SharedHandle {
    fn from(handle: OwnedHandle) -> Self {
        Self::new(handle)
    }
}

impl From<File> for SharedHandle {
    fn from(file: File) -> Self {
        Self::new(OwnedHandle::from(file))
    }
}

impl AsHandle for SharedHandle {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.0.as_handle()
    }
}

impl AsRawHandle for SharedHandle {
    fn as_raw_handle(&self) -> RawHandle {
        self.0.as_raw_handle()
    }
}

#[cfg(test)]
mod tests;
