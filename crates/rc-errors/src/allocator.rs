// Copyright (c) 2026 Mike Grier
//! The allocator interface (RCE-D-3): the smallest part of the standard library's unstable
//! `core::alloc::Allocator`, with its names and signatures, so moving to the stabilised trait is a
//! change of import. The names and signatures are the Rust project's; see the crate's `NOTICE`.

use core::alloc::Layout;
use core::error::Error;
use core::fmt;
use core::ptr::{self, NonNull};

#[cfg(test)]
mod tests;

/// An allocator could not provide the block asked for: it is out of memory, or the layout is
/// one it cannot serve.
///
/// Mirrors the standard library's unstable `core::alloc::AllocError`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct AllocError;

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("memory allocation failed")
    }
}

impl Error for AllocError {}

/// Something that allocates and frees blocks of memory.
///
/// Mirrors the standard library's unstable `core::alloc::Allocator`, keeping only `allocate` and
/// `deallocate`: this crate calls nothing else. It is used as `&'static (dyn Allocator + Sync)`,
/// so a block can be freed through the allocator that made it, from any thread.
///
/// # Safety
///
/// An implementer promises that:
///
/// - a block `allocate` returns is valid for reads and writes of its whole length, and is aligned
///   to the layout's alignment, until it is passed to `deallocate`;
/// - the block's length is at least the layout's size;
/// - `deallocate` accepts, from any thread, every block this allocator returned that has not yet
///   been freed, given the layout it was allocated with.
pub unsafe trait Allocator {
    /// Allocates a block that fits `layout`, or reports that it could not. Never panics on
    /// running out of memory.
    ///
    /// # Errors
    ///
    /// [`AllocError`] when the block cannot be provided.
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError>;

    /// Frees a block.
    ///
    /// # Safety
    ///
    /// `ptr` must be the start of a block this allocator returned and has not yet freed, and
    /// `layout` must be the layout it was allocated with.
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout);
}

/// The global allocator, as an [`Allocator`].
///
/// Mirrors the standard library's unstable `alloc::alloc::Global`. A failure is reported as an
/// [`AllocError`], never through `handle_alloc_error`. A zero-sized layout is served without
/// allocating, by a dangling pointer aligned to the layout, and freeing one does nothing.
///
/// ```
/// use core::alloc::Layout;
/// use rc_errors::{Allocator, Global};
///
/// let layout = Layout::from_size_align(64, 16).unwrap();
/// let block = Global.allocate(layout).expect("64 bytes");
/// assert!(block.len() >= 64);
/// assert_eq!(block.cast::<u8>().as_ptr().addr() % 16, 0);
/// // SAFETY: `block` came from `Global` with `layout`, and is freed once.
/// unsafe { Global.deallocate(block.cast(), layout) };
/// ```
#[derive(Copy, Clone, Default, Debug)]
pub struct Global;

// SAFETY: a nonzero-sized block comes from the global allocator, which returns memory valid and
// aligned for the layout, of at least its size, and frees it from any thread given the same
// layout. A zero-sized block is a dangling pointer at the layout's alignment, valid for the zero
// bytes it spans, and is never passed to the global allocator.
unsafe impl Allocator for Global {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let start = if layout.size() == 0 {
            NonNull::new(ptr::without_provenance_mut::<u8>(layout.align())).ok_or(AllocError)?
        } else {
            // SAFETY: the layout's size is not zero.
            NonNull::new(unsafe { alloc::alloc::alloc(layout) }).ok_or(AllocError)?
        };
        Ok(NonNull::slice_from_raw_parts(start, layout.size()))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            // SAFETY: the caller promises `ptr` is a live block from `allocate` with `layout`, and
            // a nonzero-sized one came from the global allocator with that layout.
            unsafe { alloc::alloc::dealloc(ptr.as_ptr(), layout) };
        }
    }
}
