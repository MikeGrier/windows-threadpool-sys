// Copyright (c) 2026 Mike Grier
//! The directory's calls: open it, ask it which volume it lives on, and read a
//! batch of records out of it. Everything else in the crate is bookkeeping
//! around these.
//!
//! # Every call is an entry of the request crate
//!
//! This crate makes none of these calls itself. The open is
//! `windows-namespace-request-sys`' `OpenFile`, built from the request's
//! `AnchoredPath`; the directory check is its `QueryFileBasicInfo`; the volume
//! serial is its `QueryFileIdInfo`; and a refill is its `QueryFileInformation`,
//! performed into this crate's own buffer. The directory is held as a
//! `SharedHandle` and every query is built from a clone of it, so no handle is
//! duplicated: each entry holds the very handle it was given.
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

use io_buffers::AlignedBuf;
use win_shared_os_owned_handle::SharedHandle;
use windows_impersonation_token_sys::ImpersonationToken;
use windows_namespace_request_sys::query::PerformIntoError;
use windows_namespace_request_sys::{
    AnchoredPath, FileInformationClass, OpenFile, QueryFileBasicInfo, QueryFileIdInfo,
    QueryFileInformation,
};
use windows_sys::Win32::Foundation::{
    ERROR_BAD_LENGTH, ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
    ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_NO_MORE_FILES,
    ERROR_NOT_SUPPORTED,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

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
/// using it. Those are the only flags, so the handle is not overlapped and is
/// never associated with a completion port: `GetFileInformationByHandleEx` has
/// no overlapped form, and this handle is queried synchronously on a worker.
///
/// The open's code is the request crate's: it reads the thread's last error in
/// the statement after the call, so it is taken before the guard restores the
/// thread token and nothing in between can overwrite it.
///
/// # Errors
///
/// Returns [`EnumerationError::Impersonation`] if the captured context could not
/// be applied -- in which case nothing was opened under the wrong identity --
/// and [`EnumerationError::DirectoryOpen`] with the raw code otherwise.
pub(crate) fn open_directory(
    path: &AnchoredPath,
    token: &ImpersonationToken,
) -> Result<SharedHandle, EnumerationError> {
    let open = open_request(path);

    let opened = token
        .with_impersonation(|| open.perform())
        .map_err(EnumerationError::Impersonation)?;
    let handle = SharedHandle::from(opened.map_err(EnumerationError::DirectoryOpen)?);

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

/// The open, as the request crate's entry: listing access, every share, an existing directory, and
/// backup semantics, with nothing else.
fn open_request(path: &AnchoredPath) -> OpenFile {
    OpenFile::from_anchored(path.clone())
        .with_desired_access(FILE_LIST_DIRECTORY)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(OPEN_EXISTING)
        .with_flags_and_attributes(FILE_FLAG_BACKUP_SEMANTICS)
}

/// Whether an opened handle refers to a directory.
fn is_directory(handle: &SharedHandle) -> Result<bool, EnumerationError> {
    let info = QueryFileBasicInfo::new(handle.clone())
        .perform()
        .map_err(EnumerationError::DirectoryOpen)?;
    Ok(info.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0)
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
pub(crate) fn volume_serial(directory: &SharedHandle) -> Result<u64, Win32Error> {
    QueryFileIdInfo::new(directory.clone())
        .perform()
        .map(|info| info.volume_serial_number())
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

/// The query a refill performs: the restart class for the first, the continuing class for every
/// later one, on a clone of the directory's handle.
fn refill_query(directory: &SharedHandle, which: Refill) -> QueryFileInformation {
    let class = match which {
        Refill::First => FileInformationClass::ID_EXTD_DIRECTORY_RESTART,
        Refill::Next => FileInformationClass::ID_EXTD_DIRECTORY,
    };
    QueryFileInformation::new(directory.clone(), class)
}

/// Read one batch of directory records into the buffer.
///
/// The query is the request crate's `QueryFileInformation`, built from a clone
/// of the directory's handle and performed into the buffer this crate owns, so
/// nothing is allocated and no handle is made per refill.
///
/// # Panics
///
/// Never in a release build; a failure is reported as [`RefillOutcome::Failed`]
/// so a caller cannot forget to classify it. In a debug build, panics if this
/// crate's own preconditions for that classification do not hold: a non-null
/// 8-byte-aligned buffer base, and a length that is at least
/// [`MINIMUM_BUFFER_CAPACITY`], an 8-byte multiple, and `u32`-representable.
/// Every path that reaches this function already establishes all of these
/// before calling it; a violation would be a bug in this crate, not a
/// filesystem incapability, which is exactly what
/// [`classify_refill_failure`] must never confuse the two for.
pub(crate) fn refill(
    directory: &SharedHandle,
    buffer: &mut AlignedBuf,
    which: Refill,
) -> RefillOutcome {
    let capacity = buffer.len();

    // Asserted here, once, immediately before the call whose
    // `ERROR_INVALID_FUNCTION` / `ERROR_NOT_SUPPORTED` / `ERROR_INVALID_PARAMETER`
    // failure `classify_refill_failure` reads as "this filesystem cannot do
    // extended directory information". Every one of these is already true by
    // construction on every path that reaches this function; asserting them is
    // what stops a future regression in buffer handling from being silently
    // misreported as a filesystem incapability instead of caught as this
    // crate's own bug.
    debug_assert!(
        !buffer.as_ptr().is_null(),
        "the buffer base must not be null"
    );
    debug_assert_eq!(
        buffer.as_ptr().addr() % RECORD_ALIGNMENT,
        0,
        "the buffer base must be 8-byte aligned"
    );
    debug_assert!(
        capacity >= MINIMUM_BUFFER_CAPACITY,
        "the effective capacity must be at least the minimum buffer capacity"
    );
    debug_assert_eq!(
        capacity % RECORD_ALIGNMENT,
        0,
        "the effective capacity must be an 8-byte multiple"
    );
    debug_assert!(
        u32::try_from(capacity).is_ok(),
        "the effective capacity must fit the call's DWORD"
    );

    match refill_query(directory, which).perform_into(buffer) {
        Ok(()) => RefillOutcome::Batch,
        Err(PerformIntoError::Win32(code)) => {
            match classify_refill_failure(code, which, capacity) {
                Some(error) => RefillOutcome::Failed(error),
                None => RefillOutcome::Exhausted,
            }
        }
        // The request crate refuses a buffer ahead of the call only for classes whose structure
        // page requires an alignment, and neither extended-directory class is one; this buffer is
        // 8-aligned besides. So this is unreachable, and reporting a failure beats a panic in a
        // thread-pool callback. Code zero says Windows produced none, as `engine::advance` does
        // for the other state it cannot reach.
        Err(_) => {
            debug_assert!(false, "perform_into refused a buffer this crate aligned");
            RefillOutcome::Failed(EnumerationError::DirectoryQuery(Win32Error::from_code(0)))
        }
    }
}

/// Decide what a failed refill means, or `None` when it means clean exhaustion.
///
/// The preconditions the unsupported-class mapping depends on -- an
/// 8-byte-aligned non-null base, and a clamped, aligned, `u32`-representable
/// capacity -- are asserted in [`refill`], the caller, immediately before the
/// call whose failure this classifies, which is what makes it safe to read those
/// codes as "the filesystem cannot do this" rather than "this crate asked
/// wrongly". The class is one of the two the request crate names, built there.
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
