// Copyright (c) 2026 Mike Grier
//! The fixed native staging buffer one enumeration reads into.
//!
//! # Why the element type is `u64`
//!
//! A `FILE_ID_EXTD_DIR_INFO` contains `i64` fields, and the API keeps every
//! record in a batch on an 8-byte boundary -- but only if the batch itself
//! starts on one. A `Vec<u8>` guarantees byte alignment and nothing more, and a
//! misaligned batch is not a subtle problem: the very first query fails with
//! `ERROR_NOACCESS`, and reading those fields would be undefined behaviour in
//! Rust regardless. Storing `u64` words makes the base address 8-byte aligned by
//! construction rather than by hope.
//!
//! # Why allocation is fallible
//!
//! The ordinary growable-vector path aborts the process when an allocation
//! fails. A caller that asked for a large buffer deserves an error instead, so
//! the reservation is made with `try_reserve_exact` and the buffer is only then
//! given its length -- a resize that cannot reallocate, and so cannot abort.

use crate::request::RECORD_ALIGNMENT;

/// One enumeration's reusable staging buffer.
///
/// Its capacity is fixed for the enumeration's whole life: the buffer never
/// grows in response to what a directory turns out to contain, because a bound
/// that silently moves is not a bound.
pub(crate) struct NativeBuffer {
    words: Vec<u64>,
}

impl NativeBuffer {
    /// Allocate `capacity` bytes, or report that the allocation failed.
    ///
    /// `capacity` is the effective capacity a request computed, which is already
    /// clamped, aligned, and known to fit a Win32 `u32`.
    pub(crate) fn try_new(capacity: usize) -> Option<Self> {
        debug_assert_eq!(
            capacity % RECORD_ALIGNMENT,
            0,
            "an effective capacity is always a whole number of words"
        );
        let words = capacity / RECORD_ALIGNMENT;
        let mut storage: Vec<u64> = Vec::new();
        storage.try_reserve_exact(words).ok()?;
        // Cannot reallocate, and so cannot abort: the capacity is already held.
        storage.resize(words, 0);
        Some(Self { words: storage })
    }

    /// The buffer's capacity in bytes, in the form Win32 wants it.
    pub(crate) fn capacity(&self) -> u32 {
        let bytes = self.words.len() * RECORD_ALIGNMENT;
        u32::try_from(bytes).expect("an effective capacity always fits a u32")
    }

    /// A writable pointer to the start of the buffer.
    ///
    /// 8-byte aligned by construction, which is what the directory-information
    /// classes require of a batch.
    pub(crate) fn as_mut_ptr(&mut self) -> *mut core::ffi::c_void {
        self.words.as_mut_ptr().cast()
    }

    /// The bytes the last query wrote into, as a shared slice.
    ///
    /// The whole buffer is exposed rather than the written extent, because
    /// `GetFileInformationByHandleEx` reports no written length: a batch is
    /// walked by its own next-entry offsets, and the parser (FE-9) is what
    /// bounds every read against this slice.
    #[allow(dead_code, reason = "FE-9's record parser is the reader")]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: `u64` has no padding or invalid bit patterns, so its storage
        // is always readable as initialised bytes, and the lifetime is the
        // borrow's.
        unsafe {
            core::slice::from_raw_parts(
                self.words.as_ptr().cast::<u8>(),
                self.words.len() * RECORD_ALIGNMENT,
            )
        }
    }
}

impl std::fmt::Debug for NativeBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeBuffer")
            .field("capacity", &self.capacity())
            .finish()
    }
}

#[cfg(test)]
mod tests;
