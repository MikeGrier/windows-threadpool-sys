// Copyright (c) Mike Grier.

//! The `GetFileInformationByHandleEx` entry.
//!
//! Entry 5 of the audited catalogue, and the most-called namespace operation
//! across all three audited consumers.
//!
//! # Why it is here despite being trivial to marshal
//!
//! This entry needs almost no marshaling work: its inputs are a handle, a
//! scalar class, and a buffer size, with no pointer into caller memory
//! anywhere. That invites the conclusion that it does not belong in a catalogue
//! at all. Membership is decided by whether a **blocking** namespace call needs
//! performing off the caller's thread, not by whether it is awkward to marshal
//! -- the latter test would select for our implementation convenience rather
//! than for consumer need. On the former test this is the call whose lack of an
//! overlapped form is why an unassociated handle had to become a first-class
//! destination at all.
//!
//! # This returns bytes, and does not parse them
//!
//! The audited classes have two result shapes -- fixed-size out-parameters and
//! variable-length batches -- and both collapse to one owned buffer here,
//! because this crate returns bytes plus the unaltered outcome. Per-class
//! parsing stays with the consumer that already owns it.
//!
//! Two constraints on that buffer are not negotiable:
//!
//! - It must be **8-byte aligned**. A `Vec<u8>` guarantees byte alignment and
//!   nothing more, and a misaligned batch is not a subtle problem: the very
//!   first query fails with `ERROR_NOACCESS`. [`AlignedBuffer`] states the
//!   alignment rather than arriving at it by luck.
//! - The call **reports no written length**. A batch is walked by its own
//!   next-entry offsets, so the whole buffer comes back and the consumer bounds
//!   its own reads, rather than this entry inventing a byte count it cannot
//!   know.
//!
//! # Only two classes touch the enumeration cursor
//!
//! Measured on Windows 11 Enterprise 10.0.28000, `aarch64-pc-windows-msvc`,
//! against a real directory with a deliberately small buffer:
//!
//! | Question | Measured |
//! |---|---|
//! | Does a duplicated handle share the enumeration cursor? | **Yes** -- a clean continuation |
//! | Control: do two separate opens share it? | **No** -- the second restarts |
//! | Does closing the duplicate disturb the source? | **No** |
//! | Does an interleaved `FileBasicInfo`, `FileIdInfo`, or non-`Ex` query disturb it? | **No** |
//!
//! So the contract is narrower than "handle-taking entries are hazardous":
//! **only the two directory-enumeration classes mutate the shared cursor**, and
//! every other query is a pure read that composes freely with an enumeration in
//! progress, on the same handle or on a duplicate. What follows is specific --
//! a duplicate is *not* an independent enumeration, and an independent
//! traversal needs a fresh open.
//!
//! # This is single-shot, and is not a streaming engine
//!
//! An entry covering the two directory classes otherwise looks like a second
//! implementation of a shipped streaming enumerator. It is not. This entry is
//! **single-shot**: one call, one batch, and the *client* sequences the next,
//! which is the one-entry-per-Win32-call rule applied literally. A consumer
//! wanting streaming enumeration -- with the cursor, refill loop, quanta, and
//! backpressure owned for it -- wants
//! [windows-file-enumeration-sys](https://docs.rs/windows-file-enumeration-sys)
//! and should not rebuild that loop out of single-shot calls.
//!
//! All five audited classes stay reachable here regardless, because restricting
//! them would narrow the entry for a no-consumer reason.
//!
//! # No ambient context is needed
//!
//! Access was checked at the open. That is exactly why the enumeration crate
//! applies impersonation only around `CreateFileW`, and it makes this the
//! clearest case that a request and a context are **paired at submission**
//! rather than fused.

use windows_sys::Win32::Storage::FileSystem::{
    FILE_INFO_BY_HANDLE_CLASS, FileBasicInfo, FileCaseSensitiveInfo, FileIdExtdDirectoryInfo,
    FileIdExtdDirectoryRestartInfo, FileIdInfo, GetFileInformationByHandleEx,
};

use crate::buffer::AlignedBuffer;
use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::outcome::{Outcome, perform_bool};

/// The alignment a directory-information batch requires.
///
/// A `FILE_ID_EXTD_DIR_INFO` contains `i64` fields and the API keeps every
/// record in a batch on an 8-byte boundary -- but only if the batch itself
/// starts on one. Changing this value is a breaking change.
const BATCH_ALIGNMENT: usize = align_of::<u64>();

/// Which information the query asks for.
///
/// A newtype over `FILE_INFO_BY_HANDLE_CLASS` rather than an enum, because
/// Windows defines classes this crate has never heard of and refusing them
/// would narrow the entry. The named constants are the five the audit found;
/// any other class reaches Windows unaltered through [`from_raw`](Self::from_raw).
///
/// # Example
///
/// ```
/// use windows_namespace_request_sys::query::FileInformationClass;
/// use windows_sys::Win32::Storage::FileSystem::{FileBasicInfo, FileStandardInfo};
///
/// assert_eq!(FileInformationClass::BASIC.as_raw(), FileBasicInfo);
///
/// // A class with no named constant here is still expressible.
/// let standard = FileInformationClass::from_raw(FileStandardInfo);
/// assert_eq!(standard.as_raw(), FileStandardInfo);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileInformationClass(FILE_INFO_BY_HANDLE_CLASS);

impl FileInformationClass {
    /// `FileBasicInfo`: timestamps and attributes. A pure read.
    pub const BASIC: Self = Self(FileBasicInfo);
    /// `FileIdInfo`: the volume serial and 128-bit file id. A pure read.
    pub const ID: Self = Self(FileIdInfo);
    /// `FileCaseSensitiveInfo`: whether the directory is case-sensitive. A pure
    /// read.
    pub const CASE_SENSITIVE: Self = Self(FileCaseSensitiveInfo);
    /// `FileIdExtdDirectoryInfo`: the next batch of directory entries.
    ///
    /// **Advances the enumeration cursor**, which lives in the file object and
    /// is therefore shared with every duplicate of the handle.
    pub const ID_EXTD_DIRECTORY: Self = Self(FileIdExtdDirectoryInfo);
    /// `FileIdExtdDirectoryRestartInfo`: the first batch, restarting the
    /// enumeration.
    ///
    /// **Resets the enumeration cursor**, with the same sharing consequence.
    pub const ID_EXTD_DIRECTORY_RESTART: Self = Self(FileIdExtdDirectoryRestartInfo);

    /// Wraps a raw `FILE_INFO_BY_HANDLE_CLASS`.
    #[must_use]
    pub const fn from_raw(class: FILE_INFO_BY_HANDLE_CLASS) -> Self {
        Self(class)
    }

    /// The raw class value.
    #[must_use]
    pub const fn as_raw(self) -> FILE_INFO_BY_HANDLE_CLASS {
        self.0
    }

    /// Whether this class mutates the handle's shared enumeration cursor.
    ///
    /// True only for the two directory-enumeration classes. Every other class
    /// is a pure read that composes freely with an enumeration in progress,
    /// measured rather than assumed.
    ///
    /// An unrecognised class reports `false`, because this answers "is this one
    /// of the two known cursor-moving classes", not "is this provably safe".
    ///
    /// # Example
    ///
    /// ```
    /// use windows_namespace_request_sys::query::FileInformationClass;
    ///
    /// assert!(FileInformationClass::ID_EXTD_DIRECTORY.moves_enumeration_cursor());
    /// assert!(FileInformationClass::ID_EXTD_DIRECTORY_RESTART.moves_enumeration_cursor());
    ///
    /// // Interleaving one of these with an enumeration is safe: measured, not
    /// // reasoned from the object model.
    /// assert!(!FileInformationClass::BASIC.moves_enumeration_cursor());
    /// assert!(!FileInformationClass::ID.moves_enumeration_cursor());
    /// ```
    #[must_use]
    pub const fn moves_enumeration_cursor(self) -> bool {
        self.0 == FileIdExtdDirectoryInfo || self.0 == FileIdExtdDirectoryRestartInfo
    }
}

/// An owned, marshalable parameter set for `GetFileInformationByHandleEx`.
///
/// # Example
///
/// ```
/// use std::fs;
/// use std::os::windows::io::AsHandle;
///
/// use windows_namespace_request_sys::query::{FileInformationClass, QueryFileInformation};
/// use windows_namespace_request_sys::CapturedHandle;
///
/// let path = std::env::temp_dir().join(format!("wnrs-q-{}.tmp", std::process::id()));
/// fs::write(&path, b"example")?;
/// let file = fs::File::open(&path)?;
///
/// let request = QueryFileInformation::new(
///     CapturedHandle::capture(file.as_handle())?,
///     FileInformationClass::BASIC,
/// )
/// .with_capacity(256);
///
/// // The whole buffer comes back: the call reports no written length, so the
/// // consumer bounds its own reads rather than trusting a count we invented.
/// let bytes = request.perform()?;
/// assert_eq!(bytes.len(), 256);
/// assert_eq!(bytes.as_ptr() as usize % 8, 0);
/// # drop(file);
/// # fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request queries nothing"]
pub struct QueryFileInformation {
    handle: CapturedHandle,
    class: FileInformationClass,
    capacity: usize,
}

impl QueryFileInformation {
    /// The capacity a request starts with.
    ///
    /// Large enough for any of the fixed-size classes and for a useful batch of
    /// directory entries. A caller enumerating a large directory will want more
    /// and should say so; this is a starting point, not a recommendation.
    pub const DEFAULT_CAPACITY: usize = 64 * 1024;

    /// Begins a request against `handle` for `class`.
    pub fn new(handle: CapturedHandle, class: FileInformationClass) -> Self {
        Self {
            handle,
            class,
            capacity: Self::DEFAULT_CAPACITY,
        }
    }

    /// Sets the buffer size the query is given.
    ///
    /// For a fixed-size class this must be at least the size of that class's
    /// structure, and Windows reports `ERROR_BAD_LENGTH` if it is not. For a
    /// directory class it bounds how many entries one call can return, and a
    /// buffer too small for even one entry fails with `ERROR_MORE_DATA` --
    /// neither is pre-empted here.
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// The owned duplicate of the handle being queried.
    pub fn handle(&self) -> &CapturedHandle {
        &self.handle
    }

    /// The information class this request asks for.
    #[must_use]
    pub fn class(&self) -> FileInformationClass {
        self.class
    }

    /// The buffer size the query will be given.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Copies the request, duplicating the handle.
    ///
    /// Not `Clone`, because duplicating a handle is fallible. Note that the
    /// copy shares the original's enumeration cursor, per this module's
    /// measurements -- it is a second reference, not a second enumeration.
    ///
    /// # Errors
    ///
    /// Returns the handle-capture failure when the handle cannot be duplicated.
    pub fn try_clone(&self) -> Result<Self, HandleCaptureError> {
        Ok(Self {
            handle: self.handle.try_clone()?,
            class: self.class,
            capacity: self.capacity,
        })
    }

    /// Performs the query on the calling thread.
    ///
    /// Returns the **whole** buffer, 8-byte aligned. The call reports no
    /// written length -- a batch is walked by its own next-entry offsets -- so
    /// nothing here invents a byte count it cannot know.
    ///
    /// # Errors
    ///
    /// Returns the raw Win32 code, unaltered. `ERROR_NO_MORE_FILES` ends a
    /// directory enumeration and `ERROR_MORE_DATA` means the buffer held no
    /// complete entry; both are the caller's to interpret.
    pub fn perform(&self) -> Outcome<AlignedBuffer> {
        let mut buffer = AlignedBuffer::zeroed(self.capacity, BATCH_ALIGNMENT);
        let size = u32::try_from(self.capacity).unwrap_or(u32::MAX);

        perform_bool(|| {
            // SAFETY: the handle is a duplicate this request owns and keeps
            // open across the call; the buffer is writable for `size` bytes and
            // 8-byte aligned, which is what the directory classes require.
            unsafe {
                GetFileInformationByHandleEx(
                    self.handle.raw(),
                    self.class.as_raw(),
                    buffer.as_mut_ptr().cast(),
                    size,
                )
            }
        })?;

        Ok(buffer)
    }
}

impl crate::request::Request for QueryFileInformation {
    type Error = crate::Win32Error;
    type Output = AlignedBuffer;

    fn perform(&self) -> Outcome<AlignedBuffer> {
        Self::perform(self)
    }
}

#[cfg(test)]
mod tests;
