// Copyright (c) 2026 Mike Grier
//! The coarse notification handle: `FindFirstChangeNotificationW` /
//! `FindNextChangeNotification` / `FindCloseChangeNotification` (D-17, M6.1).
//!
//! This is the universal floor beneath `ReadDirectoryChangesW`: it exists on
//! every volume that supports change notification at all, but reports no detail
//! -- an activation means only "something changed within reach", which the
//! watcher surfaces as `Desync { Coarse }` rather than pretending to know what.
//!
//! The handle needs a non-`CloseHandle` destructor, which is why it is bound to
//! the thread pool through [`WaitableHandle::assume_waitable_with`] rather than
//! [`WaitableHandle::assume_waitable`]: the latter takes a std `OwnedHandle`,
//! which the pool closes with `CloseHandle` on teardown -- the wrong routine for
//! this handle.

use std::path::Path;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FindCloseChangeNotification, FindFirstChangeNotificationW,
};
use windows_threadpool_sys::wait::WaitableHandle;

use crate::directory::{OpenError, classify, wide_path};

/// A `FindFirstChangeNotification` handle, owned until it is handed to the
/// thread pool.
///
/// Not `Send`/`Sync` by inheritance from `HANDLE` (a bare integer) needing no
/// such bound here; the type is used only long enough to be converted into a
/// [`WaitableHandle`], which does carry those bounds.
pub(crate) struct CoarseHandle {
    handle: HANDLE,
}

impl CoarseHandle {
    /// Open a coarse watch over `path`.
    ///
    /// `filter` is the same `FILE_NOTIFY_CHANGE_*` mask a detailed read would
    /// use -- the wire type is identical between the two APIs. `subtree` is
    /// fixed for the life of the handle, the same way a detailed read's is
    /// fixed for the life of *that* handle: widening either one means
    /// reopening, not reconfiguring (D-52's mechanism generalises here).
    ///
    /// # Errors
    ///
    /// Returns a classified [`OpenError`]; see `OpenFailure` for what each class
    /// means for the retry policy.
    pub(crate) fn open(path: &Path, subtree: bool, filter: u32) -> Result<Self, OpenError> {
        let wide = wide_path(path)?;
        // SAFETY: `wide`'s terminated pointer is NUL-terminated and outlives the
        // call; the interior-NUL case was already rejected by `wide_path`.
        let raw = unsafe {
            FindFirstChangeNotificationW(wide.as_terminated_ptr(), i32::from(subtree), filter)
        };
        if raw == INVALID_HANDLE_VALUE {
            let source = std::io::Error::last_os_error();
            return Err(OpenError::new(classify(&source), source));
        }
        Ok(Self { handle: raw })
    }

    /// Hand ownership to the thread pool, which closes it with
    /// `FindCloseChangeNotification` rather than `CloseHandle`.
    ///
    /// # Safety
    ///
    /// The caller must not use `self.handle` again after this call except
    /// through the returned [`WaitableHandle`]; `self` is forgotten rather than
    /// dropped so its own `Drop` does not also close the handle.
    pub(crate) unsafe fn into_waitable(self) -> WaitableHandle {
        let handle = self.handle;
        std::mem::forget(self);
        // SAFETY: forwarded from this function's own contract -- `handle` is a
        // live `FindFirstChangeNotification` handle, transferred exclusively,
        // and `FindCloseChangeNotification` is its correct destructor.
        unsafe { WaitableHandle::assume_waitable_with(handle, FindCloseChangeNotification) }
    }
}

impl std::fmt::Debug for CoarseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoarseHandle")
            .field("handle", &self.handle)
            .finish()
    }
}

impl Drop for CoarseHandle {
    fn drop(&mut self) {
        // Reached only if `into_waitable` was never called (an error path
        // between opening and binding to the pool).
        // SAFETY: `self.handle` is a live handle this value exclusively owns.
        unsafe {
            FindCloseChangeNotification(self.handle);
        }
    }
}

#[cfg(test)]
mod tests;
