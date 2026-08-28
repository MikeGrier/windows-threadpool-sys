// Copyright (c) 2026 Mike Grier
//! The documented Win32 calls this crate is built on.
//!
//! Three of them: open the directory, ask it which volume it lives on, and read
//! a batch of records out of it. Everything else in the crate is bookkeeping
//! around these.
//!
//! # Opening under someone else's identity
//!
//! The directory is opened on a thread-pool worker whose own thread token is
//! whatever the pool last left there. The submitter's captured context is
//! applied for exactly the span of the open and restored immediately afterwards
//! -- on every path, including a failed open and an unwind, because the sibling
//! crate's guard restores from its own `Drop`.
//!
//! Only the *open* runs impersonated. Access is decided when the handle is
//! created, so every later query on that handle carries the access the open
//! obtained, and re-impersonating for each of them would buy nothing while
//! putting an identity change on the hot path.
//!
//! # What counts as the end of a directory
//!
//! There are two forms, and telling them apart needs to know which query this
//! is. `ERROR_NO_MORE_FILES` ends any query. `ERROR_FILE_NOT_FOUND` ends only
//! the *first* one -- it is what an entry-less directory reports instead -- and
//! from a later query it is a genuine failure. The same code from `CreateFileW`
//! is a missing directory, which is a third meaning again. None of this is
//! guesswork the caller should have to do, so the phase is tracked here and the
//! outcome is typed.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_impersonation_token_sys::ImpersonationToken;
use windows_sys::Win32::Foundation::{
    ERROR_BAD_LENGTH, ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
    ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_NO_MORE_FILES,
    ERROR_NOT_SUPPORTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileBasicInfo,
    FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo, FileIdInfo,
    GetFileInformationByHandleEx, OPEN_EXISTING,
};
use wtf_string::Wtf16Str;

use crate::buffer::NativeBuffer;
use crate::error::{EnumerationError, Win32Error};
use crate::request::{MINIMUM_BUFFER_CAPACITY, RECORD_ALIGNMENT};

/// The access an enumeration needs, and no more.
///
/// `FILE_LIST_DIRECTORY` is the same bit as `FILE_READ_DATA`; asking for it
/// rather than `GENERIC_READ` keeps the open to what listing actually requires,
/// so a directory a caller may list but not read attributes of still opens.
const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;

/// Open one directory under the submitter's captured security context.
///
/// `FILE_FLAG_BACKUP_SEMANTICS` is what makes `CreateFileW` return a *directory*
/// handle at all; without it the call fails on any directory. Sharing is left
/// wide open because enumerating a directory must not stop anyone else from
/// using it.
///
/// # Errors
///
/// Returns [`EnumerationError::Impersonation`] if the captured context could not
/// be applied -- in which case nothing was opened under the wrong identity --
/// and [`EnumerationError::DirectoryOpen`] with the raw code otherwise.
pub(crate) fn open_directory(
    path: &Wtf16Str,
    token: &ImpersonationToken,
) -> Result<OwnedHandle, EnumerationError> {
    // Terminated for `CreateFileW`; the request already rejected interior NULs,
    // so this names exactly the path the caller asked for.
    let path = wtf_string::Wtf16String::from_units(path.as_units());

    let opened = token
        .with_impersonation(|| {
            // SAFETY: `path` is a NUL-terminated wide string that outlives the
            // call, and every other argument is a plain value.
            let handle = unsafe {
                CreateFileW(
                    path.as_terminated_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    core::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    core::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                // Read before the guard restores the thread token, so nothing
                // in between can overwrite the thread's last error.
                Err(Win32Error::last())
            } else {
                Ok(handle)
            }
        })
        .map_err(EnumerationError::Impersonation)?;

    let handle = match opened {
        // SAFETY: `CreateFileW` succeeded, so this is a live handle this call
        // exclusively owns.
        Ok(handle) => unsafe { OwnedHandle::from_raw_handle(handle as _) },
        Err(code) => return Err(EnumerationError::DirectoryOpen(code)),
    };

    // `FILE_LIST_DIRECTORY` is the same bit as `FILE_READ_DATA`, so this open
    // succeeds on an ordinary *file* too. Establishing directory-ness here,
    // rather than letting the first refill's error code decide, is what keeps
    // "you named a file" from being reported as "this filesystem cannot do
    // extended directory information" -- the refill failure codes cannot tell
    // those apart.
    if !is_directory(&handle)? {
        return Err(EnumerationError::DirectoryOpen(Win32Error::from_code(
            ERROR_DIRECTORY,
        )));
    }
    Ok(handle)
}

/// Whether an opened handle refers to a directory.
fn is_directory(handle: &OwnedHandle) -> Result<bool, EnumerationError> {
    let mut info = FILE_BASIC_INFO {
        CreationTime: 0,
        LastAccessTime: 0,
        LastWriteTime: 0,
        ChangeTime: 0,
        FileAttributes: 0,
    };
    // SAFETY: `handle` is live, and the buffer matches the requested class in
    // both type and size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle() as HANDLE,
            FileBasicInfo,
            (&raw mut info).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(EnumerationError::DirectoryOpen(Win32Error::last()));
    }
    Ok(info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0)
}

/// Ask which volume a directory lives on.
///
/// A file ID is unique only within a volume, so this is what makes an identity
/// globally meaningful. It is a separate query, which is exactly why the request
/// gets to say whether it is worth making.
///
/// # Errors
///
/// Returns the raw code. A redirector may list a directory happily and still
/// refuse this class, which is why `BestEffort` exists.
pub(crate) fn volume_serial(directory: &OwnedHandle) -> Result<u64, Win32Error> {
    let mut info = FILE_ID_INFO {
        VolumeSerialNumber: 0,
        FileId: windows_sys::Win32::Storage::FileSystem::FILE_ID_128 {
            Identifier: [0; 16],
        },
    };
    // SAFETY: `directory` is a live handle, and the buffer matches the class
    // being requested in both type and size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle() as HANDLE,
            FileIdInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(Win32Error::last());
    }
    Ok(info.VolumeSerialNumber)
}

/// Which directory-information query a refill is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refill {
    /// The first query, which also restarts the enumeration.
    ///
    /// Only this one may report an entry-less directory as
    /// `ERROR_FILE_NOT_FOUND`.
    First,
    /// Every later query.
    Next,
}

/// What one refill produced.
#[derive(Debug)]
pub(crate) enum RefillOutcome {
    /// The buffer holds a batch of records to parse.
    Batch,
    /// The directory has no more entries. Clean exhaustion, not a failure.
    Exhausted,
    /// The query failed.
    Failed(EnumerationError),
}

/// Read one batch of directory records into the buffer.
///
/// # Panics
///
/// Never in a release build; a failure is reported as [`RefillOutcome::Failed`]
/// so a caller cannot forget to classify it. In a debug build, panics if this
/// crate's own preconditions for that classification do not hold: a live
/// crate-opened handle, a valid information class, a non-null 8-byte-aligned
/// buffer base, and an effective capacity that is at least
/// [`MINIMUM_BUFFER_CAPACITY`], an 8-byte multiple, and `u32`-representable.
/// Every path that reaches this function already establishes all of these
/// before calling it; a violation would be a bug in this crate, not a
/// filesystem incapability, which is exactly what
/// [`classify_refill_failure`] must never confuse the two for.
pub(crate) fn refill(
    directory: &OwnedHandle,
    buffer: &mut NativeBuffer,
    which: Refill,
) -> RefillOutcome {
    let class = match which {
        Refill::First => FileIdExtdDirectoryRestartInfo,
        Refill::Next => FileIdExtdDirectoryInfo,
    };
    let capacity = buffer.capacity();

    // Asserted here, once, immediately before the call whose
    // `ERROR_INVALID_FUNCTION` / `ERROR_NOT_SUPPORTED` / `ERROR_INVALID_PARAMETER`
    // failure `classify_refill_failure` reads as "this filesystem cannot do
    // extended directory information". Every one of these is already true by
    // construction on every path that reaches this function; asserting them is
    // what stops a future regression in handle, class, or buffer handling from
    // being silently misreported as a filesystem incapability instead of
    // caught as this crate's own bug.
    let handle = directory.as_raw_handle() as HANDLE;
    debug_assert!(
        !handle.is_null() && handle != INVALID_HANDLE_VALUE,
        "the directory handle must be a live handle this crate opened"
    );
    debug_assert!(
        class == FileIdExtdDirectoryRestartInfo || class == FileIdExtdDirectoryInfo,
        "the information class must be one this crate's refill actually requests"
    );
    let base = buffer.as_mut_ptr();
    debug_assert!(!base.is_null(), "the buffer base must not be null");
    debug_assert_eq!(
        (base as usize) % RECORD_ALIGNMENT,
        0,
        "the buffer base must be 8-byte aligned"
    );
    debug_assert!(
        capacity as usize >= MINIMUM_BUFFER_CAPACITY,
        "the effective capacity must be at least the minimum buffer capacity"
    );
    debug_assert_eq!(
        capacity as usize % RECORD_ALIGNMENT,
        0,
        "the effective capacity must be an 8-byte multiple"
    );

    // SAFETY: `directory` is a live directory handle; the buffer is a live,
    // 8-byte-aligned allocation of exactly `capacity` bytes, which is what the
    // directory-information classes write into.
    let ok = unsafe { GetFileInformationByHandleEx(handle, class, base, capacity) };
    if ok != 0 {
        return RefillOutcome::Batch;
    }
    let code = Win32Error::last();
    match classify_refill_failure(code, which, capacity as usize) {
        Some(error) => RefillOutcome::Failed(error),
        None => RefillOutcome::Exhausted,
    }
}

/// Decide what a failed refill means, or `None` when it means clean exhaustion.
///
/// The preconditions the unsupported-class mapping depends on -- a live handle
/// this crate opened, a valid class, an 8-byte-aligned non-null base, and a
/// clamped, aligned, `u32`-representable capacity -- are asserted in
/// [`refill`], the caller, immediately before the call whose failure this
/// classifies, which is what makes it safe to read those codes as "the
/// filesystem cannot do this" rather than "this crate asked wrongly".
fn classify_refill_failure(
    code: Win32Error,
    which: Refill,
    capacity: usize,
) -> Option<EnumerationError> {
    match code.code() {
        ERROR_NO_MORE_FILES => None,
        // An entry-less directory reports this from its first query. From a
        // later one the directory has already produced records, so the same code
        // is a real failure rather than an empty listing.
        ERROR_FILE_NOT_FOUND if which == Refill::First => None,
        ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED | ERROR_INVALID_PARAMETER => {
            Some(EnumerationError::UnsupportedExtendedDirectoryInfo(code))
        }
        // The buffer could not hold one whole record. It never grows, so this is
        // reported rather than hidden behind a silent reallocation.
        ERROR_MORE_DATA | ERROR_INSUFFICIENT_BUFFER | ERROR_BAD_LENGTH => {
            Some(EnumerationError::RecordTooLarge {
                buffer_capacity: capacity,
                code,
            })
        }
        _ => Some(EnumerationError::DirectoryQuery(code)),
    }
}

#[cfg(test)]
mod tests;
