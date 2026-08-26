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
//!
//! # Directory identity is by file, not by path string
//!
//! Two subscriptions can name the same directory through different path strings
//! -- a trailing separator, a different case, a symlink hop -- and coalescing
//! (D-6) has to recognise that as *one* directory, not compare spellings. So a
//! [`DirectoryId`] is computed from the open handle itself
//! (`GetFileInformationByHandle`'s volume serial number plus file index), which
//! is stable for as long as the file exists regardless of how it was reached.

use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use wtf_string::Wtf16String;

use windows_sys::Win32::Foundation::{
    ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_INVALID_NAME,
    ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_CASE_SENSITIVE_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0,
    FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileCaseSensitiveInfo, FileIdType, GetFileInformationByHandle,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, GetVolumeInformationByHandleW,
    OPEN_EXISTING, OpenFileById, VOLUME_NAME_DOS,
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
    /// This is the downgrade-to-coarse edge (D-17), handled by the coarse
    /// fallback rather than reaching here in practice.
    Unsupported,
    /// Anything else: sharing violations, exhausted handles, a network path that
    /// is momentarily unreachable. Retryable with backoff. This is the default
    /// classification, so an unrecognised error is retried rather than treated as
    /// fatal, which is what D-14's "no terminal state" requires.
    Retryable,
    /// The path cannot be handed to Win32 at all, because it contains an interior
    /// NUL. Permanent -- see the note on the enum.
    InvalidPath,
    /// The open itself succeeded (or was retryable), but the monitor could not
    /// set up what a retryable subscription needs to keep going -- its retry
    /// timer failed to be created, vanishingly rare thread-pool resource
    /// exhaustion. Permanent for this attempt: nothing was registered, so
    /// nothing would ever fire to retry it.
    RetryUnavailable,
}

impl OpenFailure {
    /// Whether retrying the open could ever succeed.
    ///
    /// False only for the caller-input failures and the setup failure; every
    /// environmental failure is retryable, including unrecognised ones.
    pub fn is_retryable(self) -> bool {
        match self {
            OpenFailure::NotFound | OpenFailure::Unsupported | OpenFailure::Retryable => true,
            OpenFailure::NotADirectory
            | OpenFailure::InvalidPath
            | OpenFailure::RetryUnavailable => false,
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
    pub(crate) fn new(failure: OpenFailure, source: std::io::Error) -> Self {
        Self { failure, source }
    }

    /// Build an error for a condition detected before or independently of the
    /// syscall, giving it the OS error code that describes it.
    pub(crate) fn synthetic(failure: OpenFailure, code: u32) -> Self {
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

    /// The raw code behind this failure (D-79), for a client that wants more
    /// than the classification.
    pub fn code(&self) -> FailureCode {
        win32_code(&self.source)
    }

    /// This failure's classification and raw code together (D-79).
    pub(crate) fn detail(&self) -> FaultDetail {
        FaultDetail {
            failure: self.failure,
            code: self.code(),
        }
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

/// A Win32 error code or an HRESULT, kept in the currency it actually arrived in
/// (D-79).
///
/// Every failure source in this crate today (`CreateFileW`,
/// `ReadDirectoryChangesW`, `FindFirstChangeNotificationW`,
/// `GetVolumeInformationByHandleW`, `ReOpenFile`) is a classic last-error API, so
/// only [`Win32`](Self::Win32) is produced today -- but a code is kept in
/// whichever currency it actually arrived in rather than forced through
/// `HRESULT_FROM_WIN32`/`HRESULT_CODE` into a single shape, either direction of
/// which is lossy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FailureCode {
    /// A `WIN32_ERROR` from a classic last-error API.
    Win32(u32),
    /// An `HRESULT` from a COM-style API. Nothing in this crate produces one
    /// today; the variant exists so a future source does not need a breaking
    /// change to be represented.
    HResult(i32),
}

/// [`OpenFailure`]'s classification plus the raw code behind it (D-79), carried
/// by every fault report and permanent failure so a client can act on more than
/// which kind of thing failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FaultDetail {
    /// How this failure should be treated by the retry policy.
    pub failure: OpenFailure,
    /// The raw code behind it.
    pub code: FailureCode,
}

impl FaultDetail {
    /// Build a detail for a condition detected before or independently of the
    /// syscall, giving it the OS error code that describes it.
    pub(crate) fn synthetic(failure: OpenFailure, code: u32) -> Self {
        Self {
            failure,
            code: FailureCode::Win32(code),
        }
    }

    /// Build a detail for [`OpenFailure::RetryUnavailable`] from the error that
    /// actually caused it (a failure to create this crate's own retry timer),
    /// rather than reporting the fixed classification with no code at all.
    pub(crate) fn retry_unavailable(error: &std::io::Error) -> Self {
        Self {
            failure: OpenFailure::RetryUnavailable,
            code: win32_code(error),
        }
    }
}

/// The raw Win32 code behind an OS error, or `0` if it did not carry one (a
/// last-error API always sets one; this covers only a caller-fabricated
/// `io::Error` with no OS error backing it).
pub(crate) fn win32_code(error: &std::io::Error) -> FailureCode {
    let code = error.raw_os_error().unwrap_or(0);
    FailureCode::Win32(u32::try_from(code).unwrap_or(0))
}

/// Classify an OS error from an open attempt into its full [`FaultDetail`]
/// (D-79): the [`OpenFailure`] classification plus the raw code behind it.
pub(crate) fn classify_detail(error: &std::io::Error) -> FaultDetail {
    FaultDetail {
        failure: classify(error),
        code: win32_code(error),
    }
}

/// Classify an OS error from an open attempt.
///
/// Anything unrecognised is [`OpenFailure::Retryable`] by design: a watcher that
/// gives up on an error it does not know is a watcher that silently stops
/// watching. Also reused to classify an *arm* failure's unsupported-class edge
/// (D-17, M6.3): `ERROR_INVALID_FUNCTION`/`ERROR_NOT_SUPPORTED` mean the same
/// thing -- "this filesystem does not support the detailed API" -- whether they
/// arise from opening the directory or from arming a read on it.
pub(crate) fn classify(error: &std::io::Error) -> OpenFailure {
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
/// crate's own documented predicate rather than a hand-rolled scan. Reused by
/// the coarse handle (M6.1), which needs the identical NUL-terminated encoding
/// for `FindFirstChangeNotificationW`.
pub(crate) fn wide_path(path: &Path) -> Result<Wtf16String, OpenError> {
    let wide = Wtf16String::from_os_str(path.as_os_str());
    if wide.has_interior_nul() {
        return Err(OpenError::synthetic(
            OpenFailure::InvalidPath,
            ERROR_INVALID_NAME,
        ));
    }
    Ok(wide)
}

/// A directory's stable identity, independent of the path string used to open
/// it.
///
/// Two opens of the same directory -- through different spellings, a symlink, or
/// a trailing separator -- yield the same id, which is what makes coalescing by
/// directory (D-6) correct rather than an accident of string comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DirectoryId {
    volume_serial: u32,
    file_index: u64,
}

impl DirectoryId {
    /// The NTFS file reference number, in the exact form `OpenFileById`'s
    /// `FILE_ID_DESCRIPTOR` needs (M11.2/D-78) -- this is the same value
    /// `identify` already read via `GetFileInformationByHandle`, not a fresh
    /// syscall.
    pub(crate) fn file_reference(self) -> u64 {
        self.file_index
    }
}

/// Read a directory's identity from a live handle, rejecting one that turns out
/// not to be a directory.
///
/// `FILE_LIST_DIRECTORY` and `FILE_READ_DATA` are the same bit, so a plain file
/// opens perfectly happily with this crate's `CreateFileW` arguments. Without
/// this check the mistake would not surface until `ReadDirectoryChangesW` failed
/// later, where it would be misread as a transient I/O fault and retried
/// forever.
fn identify(handle: HANDLE) -> Result<DirectoryId, OpenError> {
    // SAFETY: an all-integer POD struct, so an all-zero value is valid; it is
    // fully written by the call below before it is read.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` is live for the duration of this call, and `info` is a
    // valid writable destination of the required type.
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
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
    Ok(DirectoryId {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

/// The `FILE_CASE_SENSITIVE_INFO::Flags` bit meaning "this directory is
/// case-sensitive" (`FILE_CS_FLAG_CASE_SENSITIVE_DIR`). Not exported by
/// `windows-sys`, so named here per the repo's no-bare-manifest-numeric rule.
const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x0000_0001;

/// Query whether `handle` (a live, open directory handle) is a case-sensitive
/// directory, via `GetFileInformationByHandleEx`'s `FileCaseSensitiveInfo`
/// class. `false` (case-insensitive, the overwhelmingly common case and this
/// crate's behavior before this query existed) on any failure -- an older OS
/// that predates the class (pre-Windows 10 1803), or a filesystem that does
/// not implement it -- rather than propagating an error for a query that is
/// advisory, not load-bearing for opening the directory at all.
fn is_case_sensitive_dir(handle: HANDLE) -> bool {
    let mut info = FILE_CASE_SENSITIVE_INFO { Flags: 0 };
    // SAFETY: `handle` is live for the duration of this call; `info` is a
    // valid, correctly-sized writable destination for `FileCaseSensitiveInfo`.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileCaseSensitiveInfo,
            std::ptr::from_mut(&mut info).cast(),
            u32::try_from(std::mem::size_of::<FILE_CASE_SENSITIVE_INFO>())
                .expect("this fixed, small struct's size always fits a u32"),
        )
    };
    ok != 0 && info.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0
}

/// A directory opened for change notification, owned for the life of the watch.
///
/// Closing is the `OwnedHandle`'s job, so a `DirectoryHandle` cannot outlive its
/// handle or leak it.
pub struct DirectoryHandle {
    handle: OwnedHandle,
    identity: DirectoryId,
    case_sensitive: bool,
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
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        // Computed on the owned handle, so an early return here closes it rather
        // than leaking.
        let identity = identify(handle.as_raw_handle())?;
        let case_sensitive = is_case_sensitive_dir(handle.as_raw_handle());
        Ok(Self {
            handle,
            identity,
            case_sensitive,
        })
    }

    /// Reopen the directory identified by `file_id` on the same volume as
    /// `volume_hint` (D-78/M11), rather than by path: `OpenFileById` opens by
    /// file reference number, so it is structurally incapable of landing on a
    /// different filesystem object than the one `file_id` already names --
    /// unlike a fresh `CreateFileW` against the original path, which cannot
    /// tell a recreated directory from the one this watcher started on.
    ///
    /// Measured empirically in preference to `ReOpenFile` (D-52's precedent):
    /// `ReOpenFile` against a directory handle consistently failed with
    /// `ERROR_ACCESS_DENIED` on an ordinary, unprivileged process (it needs
    /// `SeBackupPrivilege` *enabled*, not merely `FILE_FLAG_BACKUP_SEMANTICS`,
    /// which only exempts the check on a fresh `CreateFileW`). `OpenFileById`
    /// carries no such requirement here.
    ///
    /// `volume_hint` only needs to name *some* still-open handle on the same
    /// volume as `file_id` -- it is never itself the object being reopened,
    /// so it stays valid even once `file_id`'s own object is gone.
    ///
    /// # Errors
    ///
    /// Returns a classified [`OpenError`] if `OpenFileById` fails -- most
    /// often because the original object no longer exists (deleted, or its
    /// volume was ejected). That is exactly when the path-based fallback is
    /// needed.
    pub(crate) fn reopen_by_id(
        volume_hint: BorrowedHandle<'_>,
        file_id: u64,
    ) -> Result<Self, OpenError> {
        let descriptor = FILE_ID_DESCRIPTOR {
            dwSize: u32::try_from(std::mem::size_of::<FILE_ID_DESCRIPTOR>())
                .expect("this fixed, small struct's size always fits a u32"),
            Type: FileIdType,
            Anonymous: FILE_ID_DESCRIPTOR_0 {
                FileId: file_id.cast_signed(),
            },
        };
        // SAFETY: `volume_hint` is borrowed and live for the duration of this
        // call; `descriptor` is a fully initialized, valid `FILE_ID_DESCRIPTOR`
        // the callee only reads.
        let raw = unsafe {
            OpenFileById(
                volume_hint.as_raw_handle(),
                &descriptor,
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            let source = std::io::Error::last_os_error();
            return Err(OpenError::new(classify(&source), source));
        }
        // SAFETY: `OpenFileById` returned a live handle that this call
        // exclusively owns.
        let owned = unsafe { OwnedHandle::from_raw_handle(raw) };
        let identity = identify(owned.as_raw_handle())?;
        let case_sensitive = is_case_sensitive_dir(owned.as_raw_handle());
        Ok(Self {
            handle: owned,
            identity,
            case_sensitive,
        })
    }

    /// This directory's stable identity (D-6).
    pub(crate) fn identity(&self) -> DirectoryId {
        self.identity
    }

    /// Whether this directory enforces case-sensitive name matching, queried
    /// once at open time via `GetFileInformationByHandleEx`'s
    /// `FileCaseSensitiveInfo` class (PR #20 review response).
    ///
    /// Case-sensitive directories are a per-directory opt-in NTFS feature
    /// (`fsutil file setCaseSensitiveInfo`), off by default; almost every
    /// directory is the ordinary case-insensitive-but-case-preserving kind
    /// this crate has always assumed. Route matching (`route.rs`) must not
    /// fold case on the rare directory that *is* case-sensitive: on such a
    /// directory `A.txt` and `a.txt` genuinely name different files, and
    /// folding them together would route one file's changes to a route
    /// subscribed to the other.
    ///
    /// The class is only supported from Windows 10 version 1803 onward; a
    /// failure (older OS, or a filesystem that does not implement it) is
    /// treated as case-insensitive, matching this crate's behavior before
    /// this method existed and every filesystem's overwhelmingly common
    /// case.
    pub(crate) fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// This directory's volume-level identity (D-78): the filesystem name and
    /// volume label, for detecting removable media swapped for different
    /// media mounted at the same path.
    ///
    /// # Errors
    ///
    /// Returns a classified [`OpenError`] if `GetVolumeInformationByHandleW`
    /// fails.
    pub(crate) fn volume_identity(&self) -> Result<VolumeIdentity, OpenError> {
        volume_identity(self.as_raw())
    }

    /// This directory's current path, as the filesystem sees it right now,
    /// queried fresh via `GetFinalPathNameByHandleW` rather than cached from
    /// whatever string originally opened it (M11.2/D-78).
    ///
    /// `OpenFileById` reopens by file reference, which is path-independent:
    /// it keeps finding the same object even after it is moved or renamed
    /// elsewhere in the namespace. A client subscribed to a path expects to
    /// watch *that path*, not "wherever this object ends up" -- so a fast
    /// `OpenFileById` reopen must confirm the object is still where this
    /// watcher's own canonical path last recorded it before trusting it, or
    /// fall back to a path-based reopen instead.
    ///
    /// # Errors
    ///
    /// Returns a classified [`OpenError`] if `GetFinalPathNameByHandleW`
    /// fails.
    pub(crate) fn canonical_path(&self) -> Result<PathBuf, OpenError> {
        canonical_path(self.as_raw())
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

/// A directory's volume-level identity (D-78): the volume serial number,
/// plus the filesystem name and volume label kept only as descriptive data
/// for [`crate::Notification::VolumeChanged`]. Compared only when a
/// path-based reopen fallback succeeds (M11.3) -- a `ReOpenFile` success is
/// structurally guaranteed to still be on the same volume -- to notice
/// removable media swapped for different media mounted at the same path.
///
/// Equality (and therefore change detection) is on `volume_serial` alone
/// (PR #20 review response): the filesystem name and volume label are both
/// mutable (a volume can be relabeled without becoming different media, and
/// two different volumes can share a label and filesystem type), so neither
/// is a sound identity signal -- comparing them would miss a genuine media
/// swap that happens to share the old label, and would falsely report a
/// change on a mere rename. `DirectoryId`'s own volume serial is not reused
/// here because it is read from a *directory* handle rather than this
/// volume-level query, and keeping the two independent avoids coupling this
/// type's meaning to `DirectoryId`'s.
#[derive(Clone, Debug)]
pub struct VolumeIdentity {
    volume_serial: u32,
    filesystem_name: Wtf16String,
    volume_label: Wtf16String,
}

impl PartialEq for VolumeIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.volume_serial == other.volume_serial
    }
}

impl Eq for VolumeIdentity {}

impl VolumeIdentity {
    /// The volume's filesystem name (e.g. `"NTFS"`, `"FAT32"`, `"ReFS"`),
    /// lossily -- for display and logging, not for round-tripping.
    #[must_use]
    pub fn filesystem_name(&self) -> String {
        self.filesystem_name.to_string_lossy()
    }

    /// The volume's label, lossily -- for display and logging, not for
    /// round-tripping.
    #[must_use]
    pub fn volume_label(&self) -> String {
        self.volume_label.to_string_lossy()
    }

    /// Build a synthetic identity that cannot match any real volume this
    /// crate would ever read (M12.6's test seam: rigging a mismatch a real
    /// removable-media swap is not otherwise reproducible in an automated
    /// test). `volume_serial` is the only field that matters for the
    /// mismatch this seam exists to rig; the descriptive fields are for
    /// display only. Available to the crate's own tests and, via the public
    /// [`for_test`](Self::for_test) wrapper, under the `test-util` feature.
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn synthetic(volume_serial: u32, filesystem_name: &str, volume_label: &str) -> Self {
        Self {
            volume_serial,
            filesystem_name: Wtf16String::from_os_str(std::ffi::OsStr::new(filesystem_name)),
            volume_label: Wtf16String::from_os_str(std::ffi::OsStr::new(volume_label)),
        }
    }

    /// Consumer test-surface constructor (D-82), available only under the
    /// off-by-default `test-util` feature. Builds a `VolumeIdentity` for a
    /// downstream consumer synthesizing a
    /// [`VolumeChanged`](crate::Notification::VolumeChanged) notification to
    /// feed its own handler; in production a `VolumeIdentity` only ever comes
    /// from reading a real volume. Volume identity compares by serial alone, so
    /// `volume_serial` is what decides whether two identities match; the
    /// filesystem name and label are for display (D-83: valid by construction).
    #[cfg(feature = "test-util")]
    #[must_use]
    pub fn for_test(volume_serial: u32, filesystem_name: &str, volume_label: &str) -> Self {
        Self::synthetic(volume_serial, filesystem_name, volume_label)
    }
}

/// Read a handle's volume-level identity via `GetVolumeInformationByHandleW`.
fn volume_identity(handle: HANDLE) -> Result<VolumeIdentity, OpenError> {
    /// `MAX_PATH + 1`, ample for either output buffer: a volume label is
    /// capped at 32 UTF-16 units by every filesystem this crate targets, and a
    /// filesystem name ("NTFS", "FAT32", "ReFS", ...) is far shorter still.
    const BUFFER_UNITS: usize = 261;
    let mut volume_label = [0u16; BUFFER_UNITS];
    let mut filesystem_name = [0u16; BUFFER_UNITS];
    let mut volume_serial = 0u32;
    // SAFETY: both buffers are valid, correctly sized, writable destinations;
    // `volume_serial` is a valid writable `u32` destination; `handle` is live
    // for the duration of this call. The max-component-length out-param is
    // not needed here, so it alone is null.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            handle,
            volume_label.as_mut_ptr(),
            u32::try_from(volume_label.len()).expect("a fixed small buffer size fits a u32"),
            &mut volume_serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            filesystem_name.as_mut_ptr(),
            u32::try_from(filesystem_name.len()).expect("a fixed small buffer size fits a u32"),
        )
    };
    if ok == 0 {
        let source = std::io::Error::last_os_error();
        return Err(OpenError::new(classify(&source), source));
    }
    Ok(VolumeIdentity {
        volume_serial,
        filesystem_name: Wtf16String::from_units(trim_nul(&filesystem_name)),
        volume_label: Wtf16String::from_units(trim_nul(&volume_label)),
    })
}

/// The units up to (excluding) the first NUL, or the whole slice if there is
/// none -- what a fixed-size Win32 output buffer needs trimmed off before it
/// is a real string.
fn trim_nul(units: &[u16]) -> &[u16] {
    units
        .iter()
        .position(|&unit| unit == 0)
        .map_or(units, |end| &units[..end])
}

/// Read a handle's current path via `GetFinalPathNameByHandleW`, growing the
/// buffer and retrying if the path is longer than the first guess -- the
/// documented two-call convention for this API.
fn canonical_path(handle: HANDLE) -> Result<PathBuf, OpenError> {
    let mut buffer = vec![0u16; 512];
    loop {
        // SAFETY: `buffer` is a valid, writable destination of the length
        // passed; `handle` is live for the duration of this call.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buffer.as_mut_ptr(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                VOLUME_NAME_DOS | FILE_NAME_NORMALIZED,
            )
        };
        if written == 0 {
            let source = std::io::Error::last_os_error();
            return Err(OpenError::new(classify(&source), source));
        }
        let written = written as usize;
        if written < buffer.len() {
            buffer.truncate(written);
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)));
        }
        // `written` is the required length, including the NUL this call
        // would otherwise have appended; the path did not fit, so retry once
        // sized to hold it.
        buffer.resize(written, 0);
    }
}

#[cfg(test)]
mod tests;
