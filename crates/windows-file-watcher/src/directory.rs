// Copyright (c) 2026 Mike Grier
//! The owned directory handle a detailed watch reads from, and the
//! classification of the ways opening one can fail.
//!
//! `ReadDirectoryChangesW` needs a handle opened three specific ways, and all
//! three are load-bearing rather than stylistic:
//!
//! - `FILE_LIST_DIRECTORY` is the access right the read requires.
//! - `FILE_FLAG_BACKUP_SEMANTICS` is mandatory: without it `CreateFileW` refuses
//!   to open a directory at all, whatever the access mask says.
//! - `FILE_FLAG_OVERLAPPED` is what makes the handle usable with the thread
//!   pool's I/O completion seam, which is how every read is issued.
//!
//! The share mode is deliberately permissive (`READ | WRITE | DELETE`). A watcher
//! is an observer: holding a directory open must not stop anyone else reading,
//! writing, renaming, or deleting it. `FILE_SHARE_DELETE` in particular is what
//! lets the watched directory be removed out from under the watch, which is a
//! case the fault model must handle rather than prevent.
//!
//! Failures are classified rather than surfaced raw, because the retry policy is
//! driven by the *class* of failure, not the code. See [`OpenFailure`].

// Reachable publicly only under `unstable-internals`, so under default features
// the accessors used by the tests and by M3's monitor read as dead. Remove this
// with that feature, in M3, rather than letting it mask genuinely dead code.
#![allow(dead_code)]

use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use wtf_string::Wtf16String;

use windows_sys::Win32::Foundation::{
    ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_INVALID_NAME,
    ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
};

/// What a failed open means for the retry policy.
///
/// The fault model (D-14/D-15) reduces every failure to one of three actions:
/// retry the open, downgrade to coarse watching, or -- for the two variants
/// marked permanent below -- neither.
///
/// The permanent pair is worth calling out, because D-14 says there is no
/// terminal *fault* state. That holds: [`NotADirectory`](Self::NotADirectory) and
/// [`InvalidPath`](Self::InvalidPath) are not faults in the environment, they are
/// the caller naming something that cannot ever be a watched directory. Retrying
/// them would spin forever against an input that will never become valid, so they
/// are reported to the caller instead of entering the retry loop. Every failure
/// that *is* environmental stays retryable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpenFailure {
    /// Nothing exists at the path. Retryable: a watch may legitimately be set on
    /// a path that does not exist yet, or whose directory was just deleted, and
    /// the monitor waits for it to appear.
    NotFound,
    /// Something exists at the path but is not a directory. Permanent for this
    /// path -- see the note on the enum.
    NotADirectory,
    /// The volume or filesystem cannot support `ReadDirectoryChangesW` at all.
    /// This is the downgrade-to-coarse edge (D-17); the coarse fallback is built
    /// in M6 and until then this is simply reported.
    Unsupported,
    /// Anything else: sharing violations, exhausted handles, a network path that
    /// is momentarily unreachable. Retryable with backoff. This is the default
    /// classification, so an unrecognised error is retried rather than treated as
    /// fatal, which is what D-14's "no terminal state" requires.
    Retryable,
    /// The path cannot be handed to Win32 at all, because it contains an interior
    /// NUL. Permanent -- see the note on the enum.
    InvalidPath,
}

impl OpenFailure {
    /// Whether retrying the open could ever succeed.
    ///
    /// False only for the two caller-input failures; every environmental failure
    /// is retryable, including unrecognised ones.
    pub fn is_retryable(self) -> bool {
        match self {
            OpenFailure::NotFound | OpenFailure::Unsupported | OpenFailure::Retryable => true,
            OpenFailure::NotADirectory | OpenFailure::InvalidPath => false,
        }
    }
}

/// A failed open: the classification that drives policy, plus the underlying OS
/// error, kept so a diagnostic never loses the original code.
#[derive(Debug)]
pub struct OpenError {
    failure: OpenFailure,
    source: std::io::Error,
}

impl OpenError {
    fn new(failure: OpenFailure, source: std::io::Error) -> Self {
        Self { failure, source }
    }

    /// Build an error for a condition detected before or independently of the
    /// syscall, giving it the OS error code that describes it.
    fn synthetic(failure: OpenFailure, code: u32) -> Self {
        Self::new(
            failure,
            std::io::Error::from_raw_os_error(
                i32::try_from(code).expect("a WIN32_ERROR always fits an i32"),
            ),
        )
    }

    /// How this failure should be treated by the retry policy.
    pub fn failure(&self) -> OpenFailure {
        self.failure
    }

    /// The underlying OS error.
    pub fn source(&self) -> &std::io::Error {
        &self.source
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.failure, self.source)
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Classify an OS error from an open attempt.
///
/// Anything unrecognised is [`OpenFailure::Retryable`] by design: a watcher that
/// gives up on an error it does not know is a watcher that silently stops
/// watching.
fn classify(error: &std::io::Error) -> OpenFailure {
    let Some(code) = error.raw_os_error() else {
        return OpenFailure::Retryable;
    };
    let Ok(code) = u32::try_from(code) else {
        return OpenFailure::Retryable;
    };
    match code {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => OpenFailure::NotFound,
        ERROR_DIRECTORY => OpenFailure::NotADirectory,
        ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED => OpenFailure::Unsupported,
        ERROR_INVALID_NAME => OpenFailure::InvalidPath,
        _ => OpenFailure::Retryable,
    }
}

/// Encode a path as a NUL-terminated wide string.
///
/// An interior NUL is rejected rather than passed on: Win32 would stop at it and
/// silently open a *different, shorter* path than the caller named, which is a
/// correctness hole rather than a mere inconvenience. `Wtf16String` keeps content
/// and terminator distinct and reports the condition itself, so this is the
/// crate's own documented predicate rather than a hand-rolled scan.
fn wide_path(path: &Path) -> Result<Wtf16String, OpenError> {
    let wide = Wtf16String::from_os_str(path.as_os_str());
    if wide.has_interior_nul() {
        return Err(OpenError::synthetic(
            OpenFailure::InvalidPath,
            ERROR_INVALID_NAME,
        ));
    }
    Ok(wide)
}

/// A directory opened for change notification, owned for the life of the watch.
///
/// Closing is the `OwnedHandle`'s job, so a `DirectoryHandle` cannot outlive its
/// handle or leak it.
pub struct DirectoryHandle {
    handle: OwnedHandle,
}

impl DirectoryHandle {
    /// Open `path` for change notification.
    ///
    /// # Errors
    ///
    /// Returns a classified [`OpenError`]; see [`OpenFailure`] for what each
    /// class means for the retry policy.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let wide = wide_path(path)?;
        // SAFETY: `wide`'s terminated pointer is NUL-terminated and outlives the
        // call, and the interior-NUL case was rejected above so the callee sees
        // the whole path; the security attributes and template handle are null by
        // design, and the remaining arguments are constants.
        let raw = unsafe {
            CreateFileW(
                wide.as_terminated_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            let source = std::io::Error::last_os_error();
            return Err(OpenError::new(classify(&source), source));
        }
        // SAFETY: `CreateFileW` returned a live handle that this call exclusively
        // owns, so transferring it to an `OwnedHandle` is sound.
        let opened = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
        };
        opened.ensure_directory()?;
        Ok(opened)
    }

    /// Reject a handle that is not actually a directory.
    ///
    /// `FILE_LIST_DIRECTORY` and `FILE_READ_DATA` are the same bit, so a plain
    /// file opens perfectly happily with the arguments above. Without this check
    /// the mistake would not surface until `ReadDirectoryChangesW` failed later,
    /// where it would be misread as a transient I/O fault and retried forever.
    fn ensure_directory(&self) -> Result<(), OpenError> {
        // SAFETY: an all-integer POD struct, so an all-zero value is valid; it is
        // fully written by the call below before it is read.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is live and owned here, and `info` is a valid
        // writable destination of the required type.
        let ok = unsafe { GetFileInformationByHandle(self.as_raw(), &mut info) };
        if ok == 0 {
            let source = std::io::Error::last_os_error();
            return Err(OpenError::new(classify(&source), source));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(OpenError::synthetic(
                OpenFailure::NotADirectory,
                ERROR_DIRECTORY,
            ));
        }
        Ok(())
    }

    /// Borrow the handle.
    pub(crate) fn as_handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// Consume the wrapper and surrender the handle.
    ///
    /// Used to hand the directory to the thread pool's I/O object, which takes
    /// ownership of the endpoint it binds.
    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.handle
    }

    /// The raw handle, for the Win32 calls that take one.
    pub(crate) fn as_raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

impl std::fmt::Debug for DirectoryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryHandle")
            .field("handle", &self.as_raw())
            .finish()
    }
}

#[cfg(test)]
mod tests;
