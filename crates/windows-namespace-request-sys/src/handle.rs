// Copyright (c) Mike Grier.

//! Owned handle references.
//!
//! Five of the round-one entries take a handle rather than a path, so handle
//! ownership is a shared primitive rather than a detail of any one of them.
//! [`CapturedHandle`] is that primitive: it duplicates a caller's handle at
//! capture, owns the duplicate for its life, and closes it on drop.

use std::fmt;
use std::io;
use std::os::windows::io::{
    AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle, RawHandle,
};
use std::ptr;

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INVALID_HANDLE, FALSE, HANDLE,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// The handle values Windows reserves for pseudo-handles, as named constants
/// rather than bare integers.
///
/// A pseudo-handle is not a reference to a kernel object; it is a constant that
/// the calling thread resolves against *itself* at each use. Changing any value
/// here is a breaking change.
mod pseudo {
    /// `GetCurrentProcess`, and also `INVALID_HANDLE_VALUE`.
    pub const CURRENT_PROCESS: isize = -1;
    /// `GetCurrentThread`.
    pub const CURRENT_THREAD: isize = -2;
    /// Reserved by Windows; no documented producer.
    pub const RESERVED: isize = -3;
    /// `GetCurrentProcessToken`.
    pub const CURRENT_PROCESS_TOKEN: isize = -4;
    /// `GetCurrentThreadToken`.
    pub const CURRENT_THREAD_TOKEN: isize = -5;
    /// `GetCurrentThreadEffectiveToken`.
    pub const CURRENT_THREAD_EFFECTIVE_TOKEN: isize = -6;
}

/// Why a handle could not be captured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HandleCaptureFailure {
    /// The source handle was null.
    NullHandle,
    /// The source handle was `INVALID_HANDLE_VALUE`.
    ///
    /// Rejected explicitly rather than passed through, because
    /// `INVALID_HANDLE_VALUE` is *also* the current-process pseudo-handle:
    /// `DuplicateHandle` would accept it and hand back a perfectly valid handle
    /// to the current process, so an unchecked `CreateFileW` failure would be
    /// captured as a successful open of something else entirely.
    InvalidHandleValue,
    /// The source handle was one of the other Win32 pseudo-handles.
    ///
    /// A pseudo-handle names whatever the *using* thread is, so duplicating one
    /// on the caller's thread and using the result on a worker would silently
    /// change what it refers to.
    PseudoHandle,
    /// Windows refused to duplicate the source handle.
    ///
    /// A handle that has already been closed fails here, with
    /// `ERROR_INVALID_HANDLE`.
    DuplicateHandle,
}

/// A synchronous failure while capturing a caller's handle.
///
/// Duplication failure is a **construction** error by design: it is raised on
/// the calling thread, at the point the request is built, where the caller still
/// holds the source handle and can still do something about it. Deferring it to
/// execution would report a caller's mistake on a worker, to code that has no
/// way to correct it.
#[derive(Debug)]
pub struct HandleCaptureError {
    failure: HandleCaptureFailure,
    source: io::Error,
}

impl HandleCaptureError {
    fn new(failure: HandleCaptureFailure, source: io::Error) -> Self {
        Self { failure, source }
    }

    fn invalid_handle(failure: HandleCaptureFailure) -> Self {
        Self::new(
            failure,
            io::Error::from_raw_os_error(
                i32::try_from(ERROR_INVALID_HANDLE).expect("ERROR_INVALID_HANDLE fits in i32"),
            ),
        )
    }

    /// Why the capture failed.
    #[must_use]
    pub fn failure(&self) -> HandleCaptureFailure {
        self.failure
    }

    /// The underlying Win32 error code.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for HandleCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stage = match self.failure {
            HandleCaptureFailure::NullHandle => "null source handle",
            HandleCaptureFailure::InvalidHandleValue => "INVALID_HANDLE_VALUE source handle",
            HandleCaptureFailure::PseudoHandle => "pseudo-handle source handle",
            HandleCaptureFailure::DuplicateHandle => "DuplicateHandle",
        };

        write!(f, "{stage}: {}", self.source)
    }
}

impl std::error::Error for HandleCaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// An owned duplicate of a handle a request names.
///
/// # A path is copied; a handle is duplicated
///
/// This is the distinction a caller reasoning in value semantics will get
/// wrong, so it is stated rather than implied: **a path is a value and is
/// copied; a handle is a reference to a kernel object, and duplicating it
/// shares that object rather than cloning it.**
///
/// A request holding a `CapturedHandle` is therefore self-contained with
/// respect to **lifetime** -- it cannot be left pointing at a handle its
/// originator closed, because it holds its own reference and closes it on drop
/// -- and is **not** isolated with respect to **state**. Measured, not reasoned:
///
/// - A duplicate **shares directory-enumeration state**: it continues where the
///   source stopped rather than starting its own listing. An independent
///   traversal needs a fresh open, not a duplicate.
/// - Closing the duplicate **does not disturb the source**. This is what makes
///   the whole design safe: a request may own a duplicate and drop it without
///   damaging the handle its caller kept.
/// - Single-shot metadata queries disturb nothing, on the source or on a
///   duplicate.
///
/// # What is duplicated
///
/// The duplicate carries the source's access rights (`DUPLICATE_SAME_ACCESS`),
/// because a request must be able to perform exactly the call the caller opened
/// the handle for. It is **not inheritable**, so capturing a handle never
/// widens what a child process can reach.
#[derive(Debug)]
#[must_use = "dropping the captured handle closes the duplicate"]
pub struct CapturedHandle {
    duplicate: OwnedHandle,
}

impl CapturedHandle {
    /// Captures `source` by duplicating it into this process.
    ///
    /// # Errors
    ///
    /// Returns a [`HandleCaptureError`] when `source` is null, is
    /// `INVALID_HANDLE_VALUE`, is a Win32 pseudo-handle, or cannot be
    /// duplicated -- which is what an already-closed handle produces.
    pub fn capture(source: BorrowedHandle<'_>) -> Result<Self, HandleCaptureError> {
        // SAFETY: BorrowedHandle's invariant is that the handle it names stays
        // open for its borrow, which covers this call.
        unsafe { Self::capture_raw(source.as_raw_handle()) }
    }

    /// Captures a raw handle by duplicating it into this process.
    ///
    /// Prefer [`capture`](Self::capture) where an owned or borrowed handle is
    /// available. This form exists for the common case of a raw `HANDLE` that
    /// came straight back from a Win32 call and has no Rust owner yet.
    ///
    /// # Errors
    ///
    /// As [`capture`](Self::capture).
    ///
    /// # Safety
    ///
    /// `source` must remain open for the duration of this call. A handle closed
    /// concurrently may have had its value reused by another thread, in which
    /// case this captures a different kernel object rather than failing.
    pub unsafe fn capture_raw(source: RawHandle) -> Result<Self, HandleCaptureError> {
        if source.is_null() {
            return Err(HandleCaptureError::invalid_handle(
                HandleCaptureFailure::NullHandle,
            ));
        }

        match source as isize {
            pseudo::CURRENT_PROCESS => {
                return Err(HandleCaptureError::invalid_handle(
                    HandleCaptureFailure::InvalidHandleValue,
                ));
            }
            pseudo::CURRENT_THREAD
            | pseudo::RESERVED
            | pseudo::CURRENT_PROCESS_TOKEN
            | pseudo::CURRENT_THREAD_TOKEN
            | pseudo::CURRENT_THREAD_EFFECTIVE_TOKEN => {
                return Err(HandleCaptureError::invalid_handle(
                    HandleCaptureFailure::PseudoHandle,
                ));
            }
            _ => {}
        }

        let mut duplicate: HANDLE = ptr::null_mut();

        // SAFETY: GetCurrentProcess returns the current-process pseudo-handle,
        // which is exactly what DuplicateHandle wants for a same-process
        // duplication; source is a live handle per this function's contract;
        // duplicate points to writable storage. FALSE makes the duplicate
        // non-inheritable, and DUPLICATE_SAME_ACCESS makes the desired-access
        // argument ignored.
        let duplicated = unsafe {
            let process = GetCurrentProcess();
            DuplicateHandle(
                process,
                source,
                process,
                &raw mut duplicate,
                0,
                FALSE,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == FALSE {
            return Err(HandleCaptureError::new(
                HandleCaptureFailure::DuplicateHandle,
                io::Error::last_os_error(),
            ));
        }

        // SAFETY: a successful DuplicateHandle yields a new handle that this
        // process must release with CloseHandle, which OwnedHandle does.
        let duplicate = unsafe { OwnedHandle::from_raw_handle(duplicate) };
        Ok(Self { duplicate })
    }

    /// Captures a second, independently owned duplicate.
    ///
    /// This is not `Clone` because duplication is fallible. The result refers to
    /// the *same* kernel object, with everything that implies above.
    ///
    /// # Errors
    ///
    /// As [`capture`](Self::capture), though only
    /// [`HandleCaptureFailure::DuplicateHandle`] is reachable: the value being
    /// duplicated is already known to be a real, open handle.
    pub fn try_clone(&self) -> Result<Self, HandleCaptureError> {
        Self::capture(self.duplicate.as_handle())
    }

    /// Releases the duplicate to the caller.
    ///
    /// The handle stays open; ownership moves.
    #[must_use]
    pub fn into_owned_handle(self) -> OwnedHandle {
        self.duplicate
    }
}

impl AsHandle for CapturedHandle {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.duplicate.as_handle()
    }
}

impl From<CapturedHandle> for OwnedHandle {
    fn from(captured: CapturedHandle) -> Self {
        captured.into_owned_handle()
    }
}

// Visible to the crate's own cross-module tests, which reuse this module's
// fixture rather than standing up a second copy of it.
#[cfg(test)]
pub(crate) mod tests;
