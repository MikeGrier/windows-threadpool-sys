// Copyright (c) Mike Grier.

//! The `FindFirstChangeNotificationW` entry.
//!
//! Entry 3 of the audited catalogue. It watches a directory for a class of
//! changes and produces a handle that becomes signalled when one occurs.
//!
//! # The handle it produces is not an ordinary handle
//!
//! A change-notification handle is closed with `FindCloseChangeNotification`,
//! **not** `CloseHandle`. Passing it to `CloseHandle` is a resource leak that
//! nothing reports: the call may even appear to succeed. That is why this entry
//! returns [`ChangeNotification`] rather than a bare `OwnedHandle` -- the type
//! is what remembers which routine closes it, so a caller cannot forget.
//!
//! # What it does and does not tell you
//!
//! The handle signals that *something* in the watched set changed. It carries
//! no record of what, and a burst of changes may signal once. A consumer
//! needing the individual changes wants `ReadDirectoryChangesW`, which is a
//! different call and therefore a different entry. This one is the cheap
//! "something happened, go look" primitive, and the crate does not blur them.

use std::ffi::c_void;
use std::fmt;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_ACCESS,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY, FILE_NOTIFY_CHANGE_SIZE,
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
};

use crate::outcome::{Outcome, perform_bool, perform_handle};
use crate::path::PreparedPath;

/// An owned change-notification handle.
///
/// Closed with `FindCloseChangeNotification` on drop. The type exists so that
/// the close routine travels with the handle rather than being something each
/// call site has to remember, which is the same shape
/// [windows-threadpool-sys](https://docs.rs/windows-threadpool-sys) already
/// needed for wait targets.
#[derive(Debug)]
#[must_use = "dropping the notification stops the watch"]
pub struct ChangeNotification {
    /// Always a live handle produced by `FindFirstChangeNotificationW`, until
    /// `Drop` closes it.
    handle: HANDLE,
}

impl ChangeNotification {
    /// The raw handle, for passing to a wait.
    ///
    /// Borrowed, not transferred: it stays owned by this value. Do **not** pass
    /// it to `CloseHandle`.
    #[must_use]
    pub fn as_raw(&self) -> HANDLE {
        self.handle
    }

    /// Rearms the watch after it has signalled.
    ///
    /// A notification handle signals once and then stays signalled until it is
    /// rearmed, so a consumer that waits in a loop must call this between
    /// waits.
    ///
    /// # Errors
    ///
    /// Returns the raw Win32 code, unaltered.
    pub fn rearm(&self) -> Outcome<()> {
        // SAFETY: the handle is live for this value's lifetime.
        perform_bool(|| unsafe { FindNextChangeNotification(self.handle) })
    }
}

impl Drop for ChangeNotification {
    fn drop(&mut self) {
        // SAFETY: the handle came from FindFirstChangeNotificationW and has not
        // been closed, since only this Drop closes it. The result is
        // deliberately ignored: a close failure during drop has nowhere to go,
        // and this crate does not panic on it.
        unsafe {
            FindCloseChangeNotification(self.handle);
        }
    }
}

// SAFETY: the value owns its handle exclusively and has no interior
// mutability. A Windows handle is process-wide rather than thread-affine, so
// moving it between threads and sharing a shared reference are both sound; the
// raw pointer is what blocks the automatic derivation.
unsafe impl Send for ChangeNotification {}
// SAFETY: as above. `rearm` takes `&self` and is a single Win32 call that
// Windows serialises internally.
unsafe impl Sync for ChangeNotification {}

/// Which changes a watch reports.
///
/// A thin newtype over `FILE_NOTIFY_CHANGE` rather than an enum, because the
/// value is a bitmask and Windows may define bits this crate has not heard of.
/// The named constants are provided for convenience; an unknown bit still
/// reaches Windows unaltered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NotifyFilter(FILE_NOTIFY_CHANGE);

impl NotifyFilter {
    /// An empty filter, which watches for nothing.
    ///
    /// Windows rejects this at the call, which is left to happen rather than
    /// pre-empted here.
    pub const NONE: Self = Self(0);
    /// File name creation, deletion, or rename.
    pub const FILE_NAME: Self = Self(FILE_NOTIFY_CHANGE_FILE_NAME);
    /// Directory creation or deletion.
    pub const DIR_NAME: Self = Self(FILE_NOTIFY_CHANGE_DIR_NAME);
    /// Attribute changes.
    pub const ATTRIBUTES: Self = Self(FILE_NOTIFY_CHANGE_ATTRIBUTES);
    /// Size changes, reported when the file is flushed rather than on write.
    pub const SIZE: Self = Self(FILE_NOTIFY_CHANGE_SIZE);
    /// Last-write-time changes, likewise reported on flush.
    pub const LAST_WRITE: Self = Self(FILE_NOTIFY_CHANGE_LAST_WRITE);
    /// Last-access-time changes.
    pub const LAST_ACCESS: Self = Self(FILE_NOTIFY_CHANGE_LAST_ACCESS);
    /// Creation-time changes.
    pub const CREATION: Self = Self(FILE_NOTIFY_CHANGE_CREATION);
    /// Security-descriptor changes.
    pub const SECURITY: Self = Self(FILE_NOTIFY_CHANGE_SECURITY);

    /// Wraps a raw `FILE_NOTIFY_CHANGE` mask.
    ///
    /// Any bit pattern is accepted: the crate does not decide which bits
    /// Windows understands.
    #[must_use]
    pub const fn from_bits(bits: FILE_NOTIFY_CHANGE) -> Self {
        Self(bits)
    }

    /// The raw mask.
    #[must_use]
    pub const fn bits(self) -> FILE_NOTIFY_CHANGE {
        self.0
    }

    /// The union of two filters.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit of `other` is set in this filter.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for NotifyFilter {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

/// An owned, marshalable parameter set for `FindFirstChangeNotificationW`.
///
/// # Example
///
/// ```
/// use windows_namespace_request_sys::prepare;
/// use windows_namespace_request_sys::watch::{NotifyFilter, WatchDirectory};
/// use wtf_string::Wtf16String;
///
/// let directory = std::env::temp_dir();
/// let text = directory.to_str().expect("the temporary directory is valid UTF-8");
///
/// let request = WatchDirectory::new(prepare(&Wtf16String::from(text))?)
///     .with_subtree(true)
///     .with_filter(NotifyFilter::FILE_NAME | NotifyFilter::DIR_NAME);
///
/// let notification = request.perform()?;
/// // The handle is closed with FindCloseChangeNotification, which the type
/// // remembers on the caller's behalf.
/// drop(notification);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
#[must_use = "an unperformed request watches nothing"]
pub struct WatchDirectory {
    path: PreparedPath,
    subtree: bool,
    filter: NotifyFilter,
}

impl WatchDirectory {
    /// Begins a request to watch `path`.
    ///
    /// The watch starts non-recursive and watching for nothing; both are set
    /// explicitly, as everywhere else in this crate.
    pub fn new(path: PreparedPath) -> Self {
        Self {
            path,
            subtree: false,
            filter: NotifyFilter::NONE,
        }
    }

    /// Sets `bWatchSubtree`.
    pub fn with_subtree(mut self, subtree: bool) -> Self {
        self.subtree = subtree;
        self
    }

    /// Sets `dwNotifyFilter`.
    pub fn with_filter(mut self, filter: NotifyFilter) -> Self {
        self.filter = filter;
        self
    }

    /// The prepared path this request will watch.
    #[must_use]
    pub fn path(&self) -> &PreparedPath {
        &self.path
    }

    /// Whether the watch covers the whole subtree.
    #[must_use]
    pub fn subtree(&self) -> bool {
        self.subtree
    }

    /// The change classes the watch reports.
    #[must_use]
    pub fn filter(&self) -> NotifyFilter {
        self.filter
    }

    /// Performs the call on the calling thread.
    ///
    /// # Errors
    ///
    /// Returns the raw Win32 code, unaltered.
    pub fn perform(&self) -> Outcome<ChangeNotification> {
        let raw = perform_handle(|| {
            // SAFETY: the path is NUL-terminated and outlives the call, and
            // both remaining arguments are plain values.
            unsafe {
                FindFirstChangeNotificationW(
                    self.path.as_wtf16_terminated(),
                    i32::from(self.subtree),
                    self.filter.bits(),
                )
            }
        })?;

        Ok(ChangeNotification {
            handle: raw.cast::<c_void>(),
        })
    }
}

impl fmt::Display for NotifyFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FILE_NOTIFY_CHANGE({:#x})", self.0)
    }
}

impl crate::request::Request for WatchDirectory {
    type Output = ChangeNotification;

    fn perform(&self) -> Outcome<ChangeNotification> {
        Self::perform(self)
    }
}

#[cfg(test)]
mod tests;
