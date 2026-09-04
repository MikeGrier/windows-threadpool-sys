// Copyright (c) 2026 Mike Grier
//! What a caller submits: one directory, one predicate, one set of bounds.
//!
//! A request is a plain owned value. It borrows nothing from the caller, so the
//! string it was built from may go away immediately, and it can be built on one
//! thread and submitted from another. Everything that can be rejected about a
//! request is rejected here, on the caller's own thread, before anything has
//! been accepted.

use std::path::Path;

use wtf_string::{Wtf16Str, Wtf16String};

use crate::entry::FileIdentityMode;
use crate::error::{RequestError, RequestFailure};
use crate::path;
use crate::predicate::EntryPredicate;

/// The default native buffer capacity, in bytes.
///
/// Large enough that an ordinary directory is read in one or two queries, which
/// is what keeps the per-refill cost off the per-entry path.
pub const DEFAULT_BUFFER_CAPACITY: usize = 64 * 1024;

/// The smallest native buffer capacity, in bytes.
///
/// A smaller buffer would not reliably hold one maximum-length record, turning
/// an ordinary directory into an oversize-record failure.
pub const MINIMUM_BUFFER_CAPACITY: usize = 1024;

/// The alignment a `FILE_ID_EXTD_DIR_INFO` record's fields require.
///
/// The record contains `i64` fields, and the API keeps every record in a batch
/// on this boundary, so both the buffer's base address and its length are held
/// to it.
pub(crate) const RECORD_ALIGNMENT: usize = 8;

/// The page size every Windows target this crate builds for uses.
///
/// Named rather than written as `4096` at the assertion site, and stated here
/// because it is the unit [`DEFAULT_BUFFER_CAPACITY`] is chosen in: a buffer
/// that is a whole number of pages has each refill begin on a page boundary.
/// Large-page allocations are a different mechanism and are not what this
/// buffer uses.
const PAGE_SIZE: usize = 4096;

// The relationships these capacities depend on, checked by the compiler rather
// than by a test -- they are facts about constants, so a test could only report
// after the fact, on a build somebody chose to run.
//
// A mutation run replaced `64 * 1024` with `64 + 1024`, and every test passed:
// 1088 is still above the minimum and still a legal capacity, so nothing that
// merely enumerates a directory can tell the difference.
//
// **The first version of this guard asserted `is_power_of_two`, and its comment
// gave the wrong reason.** It said 1088 is not "a whole number of records' worth
// of aligned buffer" -- but 1088 is 136 * 8, so it is exactly that. The
// assertion killed the mutant only because 1088 happens not to be a power of
// two, which is incidental: 2048 is a power of two, is not a whole page, and
// would have passed the guard while regressing every refill. Raised in the
// PR #56 review.
//
// **And the first repair over-claimed in turn**, which the same review caught:
// it said a page-multiple length makes "each refill begin on a page boundary".
// It does not. `NativeBuffer` stores `Vec<u64>`, so the allocation is 8-byte
// aligned and nothing more -- a length that is a whole number of pages says
// nothing about where the buffer starts. What the constraint actually buys is a
// page-sized *request*: a round amount for the kernel to fill per call, and one
// that does not leave a part-page tail. Alignment would need page-aligned
// storage, which this buffer does not have and does not need.
const _: () = {
    assert!(
        DEFAULT_BUFFER_CAPACITY.is_multiple_of(PAGE_SIZE),
        "the default is a whole number of pages in length; a value that is merely \
         'big enough' would pass every functional test while asking the kernel for \
         a ragged part-page amount on every refill"
    );
    assert!(
        DEFAULT_BUFFER_CAPACITY.is_power_of_two(),
        "kept alongside the page check because it is the stronger statement of \
         the same intent: the default is a round size, not an arbitrary one that \
         happens to divide by the page"
    );
    assert!(
        DEFAULT_BUFFER_CAPACITY > MINIMUM_BUFFER_CAPACITY,
        "the default must leave real headroom over the floor, or the two serve \
         the same purpose and one of them is a lie"
    );
    assert!(
        MINIMUM_BUFFER_CAPACITY.is_multiple_of(RECORD_ALIGNMENT),
        "every capacity is handed to Win32 as a buffer length, and a length that \
         is not a whole number of alignments cannot hold a whole final record"
    );
    assert!(
        DEFAULT_BUFFER_CAPACITY.is_multiple_of(RECORD_ALIGNMENT),
        "as above, for the capacity almost every caller actually uses"
    );
};

/// One directory to enumerate, with the predicate and bounds that apply to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumerationRequest {
    path: Wtf16String,
    predicate: EntryPredicate,
    file_identity_mode: FileIdentityMode,
    buffer_capacity: usize,
}

impl EnumerationRequest {
    /// Build a request for a native WTF-16 path.
    ///
    /// The path is validated and, unless it is already a verbatim `\\?\` path,
    /// resolved to its fully qualified form now. The stored value is exactly
    /// what a worker will later open.
    ///
    /// # Errors
    ///
    /// Returns [`RequestError`] for an empty path, an interior NUL, a `\\?\`
    /// path that is not fully qualified, an ordinary path longer than
    /// `MAX_PATH` before or after resolution, or a resolution failure reported
    /// by Windows.
    pub fn new(path: &Wtf16Str) -> Result<Self, RequestError> {
        Ok(Self {
            path: path::prepare(path)?,
            predicate: EntryPredicate::default(),
            file_identity_mode: FileIdentityMode::default(),
            buffer_capacity: DEFAULT_BUFFER_CAPACITY,
        })
    }

    /// Build a request for a `std` path.
    ///
    /// The conversion is lossless in both directions: a Windows [`Path`] is
    /// already WTF-16 underneath.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn for_path(path: &Path) -> Result<Self, RequestError> {
        Self::new(&Wtf16String::from_os_str(path.as_os_str()))
    }

    /// Set the predicate entries must satisfy to be delivered.
    ///
    /// Infallible because a [`QueryByExample`](crate::QueryByExample) validates
    /// each clause as it is added, so an invalid predicate cannot be built in
    /// the first place.
    #[must_use]
    pub fn with_predicate(mut self, predicate: impl Into<EntryPredicate>) -> Self {
        self.predicate = predicate.into();
        self
    }

    /// Set how much work the request will do for file identity.
    #[must_use]
    pub fn with_file_identity(mut self, mode: FileIdentityMode) -> Self {
        self.file_identity_mode = mode;
        self
    }

    /// Set the native buffer capacity, in bytes.
    ///
    /// The value is clamped up to [`MINIMUM_BUFFER_CAPACITY`] and then rounded
    /// up to the record alignment. The result is fixed for the request's whole
    /// life: the buffer never grows in response to what a directory turns out to
    /// contain, because a bound that silently moves is not a bound. Read the
    /// value back with [`buffer_capacity`](Self::buffer_capacity).
    ///
    /// # Errors
    ///
    /// Returns [`RequestFailure::BufferCapacityUnrepresentable`] if the aligned
    /// capacity cannot be passed to Win32 as a `u32`.
    pub fn with_buffer_capacity(mut self, bytes: usize) -> Result<Self, RequestError> {
        self.buffer_capacity = effective_buffer_capacity(bytes)?;
        Ok(self)
    }

    /// The exact path a worker will open.
    #[must_use]
    pub fn path(&self) -> &Wtf16Str {
        &self.path
    }

    /// The predicate entries must satisfy.
    #[must_use]
    pub fn predicate(&self) -> &EntryPredicate {
        &self.predicate
    }

    /// How much work the request will do for file identity.
    #[must_use]
    pub fn file_identity_mode(&self) -> FileIdentityMode {
        self.file_identity_mode
    }

    /// The effective native buffer capacity, in bytes, after clamping and
    /// alignment.
    #[must_use]
    pub fn buffer_capacity(&self) -> usize {
        self.buffer_capacity
    }
}

/// Clamp and align a requested capacity.
fn effective_buffer_capacity(bytes: usize) -> Result<usize, RequestError> {
    let clamped = bytes.max(MINIMUM_BUFFER_CAPACITY);
    // Rounding up cannot overflow in practice, but a `usize::MAX` request would
    // wrap to zero, which is exactly the value the alignment is meant to rule
    // out -- so the overflow is reported rather than wrapped.
    let aligned = clamped
        .checked_next_multiple_of(RECORD_ALIGNMENT)
        .ok_or_else(|| RequestError::new(RequestFailure::BufferCapacityUnrepresentable))?;
    u32::try_from(aligned)
        .map_err(|_| RequestError::new(RequestFailure::BufferCapacityUnrepresentable))?;
    Ok(aligned)
}

#[cfg(test)]
mod tests;
