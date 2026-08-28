// Copyright (c) Mike Grier.

//! An owned byte buffer with a guaranteed alignment.
//!
//! Two unrelated parts of this crate need one, for the same underlying reason:
//! a Win32 structure that a buffer merely *contains* still has to be aligned as
//! though the buffer were that structure. A self-relative security descriptor
//! requires DWORD alignment, and the directory-information classes require
//! 8-byte alignment. A `Box<[u8]>` guarantees neither -- its alignment is 1 --
//! so the requirement is met explicitly here rather than assumed twice.

use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::fmt;
use std::ptr::NonNull;
use std::slice;

/// An owned, zero-initialised byte buffer aligned to a stated boundary.
///
/// The alignment is a property of the buffer, not of the first thing written
/// into it: it holds for the buffer's whole life and is preserved by
/// [`Clone`].
pub struct AlignedBuffer {
    /// Always non-null and aligned to `layout.align()`. When `layout.size()` is
    /// zero this is a dangling-but-aligned pointer that is never dereferenced
    /// and never freed.
    pointer: NonNull<u8>,
    layout: Layout,
}

impl AlignedBuffer {
    /// Allocates `len` zeroed bytes aligned to `align`.
    ///
    /// # Panics
    ///
    /// Panics if `align` is not a power of two, or if `len` rounded up to
    /// `align` overflows `isize` -- both of which are programming errors rather
    /// than conditions a caller can encounter with valid input.
    #[must_use]
    pub fn zeroed(len: usize, align: usize) -> Self {
        let layout = Layout::from_size_align(len, align)
            .expect("an alignment that is a power of two, and a size that does not overflow");

        if len == 0 {
            // A zero-sized allocation is undefined, so use the alignment itself
            // as the address: non-null, correctly aligned, never dereferenced,
            // and never freed.
            let pointer = NonNull::new(align as *mut u8).expect("a non-zero alignment");
            return Self { pointer, layout };
        }

        // SAFETY: layout has a non-zero size, which is alloc_zeroed's only
        // requirement beyond a valid layout.
        let raw = unsafe { alloc_zeroed(layout) };
        let Some(pointer) = NonNull::new(raw) else {
            handle_alloc_error(layout);
        };

        Self { pointer, layout }
    }

    /// Allocates a buffer aligned to `align` holding a copy of `bytes`.
    ///
    /// # Panics
    ///
    /// As [`zeroed`](Self::zeroed).
    #[must_use]
    pub fn from_bytes(bytes: &[u8], align: usize) -> Self {
        let mut buffer = Self::zeroed(bytes.len(), align);
        buffer.as_mut_slice().copy_from_slice(bytes);
        buffer
    }

    /// The buffer's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.layout.size()
    }

    /// Whether the buffer holds no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The alignment the buffer's address is guaranteed to satisfy.
    #[must_use]
    pub fn align(&self) -> usize {
        self.layout.align()
    }

    /// The buffer's contents.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: pointer is valid for layout.size() initialised bytes, and the
        // borrow ties the slice to this buffer.
        unsafe { slice::from_raw_parts(self.pointer.as_ptr(), self.layout.size()) }
    }

    /// The buffer's contents, mutably.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as as_slice, and the exclusive borrow rules out aliasing.
        unsafe { slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) }
    }

    /// The buffer's address, for handing to a Win32 call.
    #[must_use]
    pub fn as_ptr(&self) -> *const u8 {
        self.pointer.as_ptr()
    }

    /// The buffer's address, for a Win32 call that writes into it.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.pointer.as_ptr()
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        if self.layout.size() == 0 {
            return;
        }

        // SAFETY: pointer came from alloc_zeroed with exactly this layout, and
        // Drop runs once.
        unsafe { dealloc(self.pointer.as_ptr(), self.layout) };
    }
}

impl Clone for AlignedBuffer {
    fn clone(&self) -> Self {
        Self::from_bytes(self.as_slice(), self.align())
    }
}

impl fmt::Debug for AlignedBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlignedBuffer")
            .field("len", &self.len())
            .field("align", &self.align())
            .finish_non_exhaustive()
    }
}

impl PartialEq for AlignedBuffer {
    /// Compares contents and alignment, not addresses.
    fn eq(&self, other: &Self) -> bool {
        self.align() == other.align() && self.as_slice() == other.as_slice()
    }
}

impl Eq for AlignedBuffer {}

// SAFETY: the buffer owns its allocation exclusively and holds plain bytes with
// no interior mutability, so moving it between threads and sharing a shared
// reference are both sound. The raw pointer is what blocks the automatic
// derivation, and it is an owning pointer rather than a borrow.
unsafe impl Send for AlignedBuffer {}
// SAFETY: as above.
unsafe impl Sync for AlignedBuffer {}

#[cfg(test)]
mod tests;
