// Copyright (c) 2026 Mike Grier
//! An owned, aligned buffer for I/O.

use std::alloc::{self, Layout};
use std::fmt;
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::slice;

use crate::{IoBuf, IoBufMut};

/// An owned, zero-initialised buffer whose address is aligned to a boundary the caller chooses.
///
/// Unbuffered file I/O needs buffer addresses aligned to the device's sector size, and a `Vec<u8>`
/// guarantees an alignment of one. This is the buffer that guarantees the alignment asked for, for
/// the buffer's whole life, and it implements [`IoBuf`] and [`IoBufMut`], so it can be handed to
/// whatever takes those. Constructing and using one needs no `unsafe`.
///
/// The alignment is a property of the address, not of the length: the length need not be a multiple
/// of it, though an I/O that needs a whole number of sectors needs a length that is, and that is
/// the caller's to choose.
///
/// A buffer of length zero allocates nothing. Its address is still non-null and aligned.
///
/// # Example
///
/// ```
/// use io_buffers::{AlignedBuf, IoBuf, IoBufMut};
///
/// // A 4 KiB sector's worth of buffer, at an address a sector-aligned write can use.
/// let mut buffer = AlignedBuf::zeroed(4096, 4096)?;
/// assert_eq!(buffer.stable_ptr() as usize % 4096, 0);
/// assert_eq!(buffer.bytes_len(), 4096);
/// assert!(buffer.iter().all(|&byte| byte == 0));
///
/// buffer[..5].copy_from_slice(b"hello");
/// assert_eq!(buffer.stable_mut_ptr().cast_const(), buffer.stable_ptr());
///
/// // The address does not change when the value moves, which is the promise an I/O layer holds
/// // the buffer to.
/// let address = buffer.stable_ptr();
/// let moved = buffer;
/// assert_eq!(moved.stable_ptr(), address);
/// # Ok::<(), io_buffers::AlignedBufError>(())
/// ```
pub struct AlignedBuf {
    /// Always non-null and aligned to `layout.align()`. When `layout.size()` is zero this is a
    /// dangling but aligned address that is never dereferenced and never freed.
    pointer: NonNull<u8>,
    layout: Layout,
}

/// Why an [`AlignedBuf`] could not be made.
///
/// Non-exhaustive, so a reason can be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AlignedBufError {
    /// The alignment is not a power of two, zero included.
    InvalidAlignment {
        /// The alignment asked for.
        align: usize,
    },
    /// The length, rounded up to the alignment, does not fit in an `isize`, which is the most any
    /// allocation can be.
    TooLarge {
        /// The length asked for.
        len: usize,
        /// The alignment asked for.
        align: usize,
    },
    /// The allocator could not provide the memory.
    AllocationFailed {
        /// The length asked for.
        len: usize,
        /// The alignment asked for.
        align: usize,
    },
}

impl fmt::Display for AlignedBufError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidAlignment { align } => {
                write!(f, "alignment {align} is not a power of two")
            }
            Self::TooLarge { len, align } => write!(
                f,
                "a buffer of {len} bytes aligned to {align} is larger than any allocation can be"
            ),
            Self::AllocationFailed { len, align } => write!(
                f,
                "the allocator could not provide {len} bytes aligned to {align}"
            ),
        }
    }
}

impl std::error::Error for AlignedBufError {}

/// The layout of a buffer of `len` bytes aligned to `align`, or why there is none.
fn layout(len: usize, align: usize) -> Result<Layout, AlignedBufError> {
    if !align.is_power_of_two() {
        return Err(AlignedBufError::InvalidAlignment { align });
    }
    Layout::from_size_align(len, align).map_err(|_| AlignedBufError::TooLarge { len, align })
}

impl AlignedBuf {
    /// Allocate `len` zeroed bytes aligned to `align`.
    ///
    /// # Errors
    ///
    /// [`AlignedBufError::InvalidAlignment`] for an alignment that is not a power of two,
    /// [`AlignedBufError::TooLarge`] for a length that cannot be allocated at that alignment, and
    /// [`AlignedBufError::AllocationFailed`] when the allocator has no memory for it. An allocation
    /// failure is reported rather than aborting the process, since a consumer sizing buffers from
    /// a device's geometry is the one who can recover from asking for too much.
    pub fn zeroed(len: usize, align: usize) -> Result<Self, AlignedBufError> {
        let layout = layout(len, align)?;
        if layout.size() == 0 {
            let address = NonZeroUsize::new(layout.align()).expect("an alignment is at least one");
            return Ok(Self {
                pointer: NonNull::without_provenance(address),
                layout,
            });
        }
        // SAFETY: the layout has a non-zero size, which is `alloc_zeroed`'s one requirement beyond
        // a valid layout.
        let raw = unsafe { alloc::alloc_zeroed(layout) };
        let pointer = NonNull::new(raw).ok_or(AlignedBufError::AllocationFailed { len, align })?;
        Ok(Self { pointer, layout })
    }

    /// Allocate a buffer aligned to `align` holding a copy of `bytes`.
    ///
    /// # Errors
    ///
    /// As [`zeroed`](Self::zeroed).
    pub fn from_bytes(bytes: &[u8], align: usize) -> Result<Self, AlignedBufError> {
        let mut buffer = Self::zeroed(bytes.len(), align)?;
        buffer.as_mut_slice().copy_from_slice(bytes);
        Ok(buffer)
    }

    /// A second buffer with the same contents and the same alignment.
    ///
    /// There is no `Clone`: copying an I/O buffer is an allocation that can fail, and one worth
    /// seeing at the call site.
    ///
    /// # Errors
    ///
    /// As [`zeroed`](Self::zeroed), for the allocation of the copy.
    pub fn try_clone(&self) -> Result<Self, AlignedBufError> {
        Self::from_bytes(self.as_slice(), self.align())
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

    /// The alignment the buffer's address satisfies, for the buffer's whole life.
    #[must_use]
    pub fn align(&self) -> usize {
        self.layout.align()
    }

    /// The buffer's contents.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the pointer is valid for `len` initialised bytes -- zeroed or copied at
        // construction -- or is a non-null aligned address for a length of zero, and the borrow
        // ties the slice to this buffer.
        unsafe { slice::from_raw_parts(self.pointer.as_ptr(), self.layout.size()) }
    }

    /// The buffer's contents, mutably.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as `as_slice`, and the exclusive borrow rules out aliasing.
        unsafe { slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) }
    }

    /// The buffer's address.
    #[must_use]
    pub fn as_ptr(&self) -> *const u8 {
        self.pointer.as_ptr()
    }

    /// The buffer's address, for a call that writes into it.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.pointer.as_ptr()
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        if self.layout.size() == 0 {
            return;
        }
        // SAFETY: a non-empty buffer's pointer came from `alloc_zeroed` with exactly this layout,
        // and `drop` runs once.
        unsafe { alloc::dealloc(self.pointer.as_ptr(), self.layout) };
    }
}

impl Deref for AlignedBuf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl AsRef<[u8]> for AlignedBuf {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsMut<[u8]> for AlignedBuf {
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl fmt::Debug for AlignedBuf {
    /// Shows the length and the alignment, never the contents: an I/O buffer routinely holds
    /// someone's data, and a `Debug` that printed it would put that data anywhere a caller logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlignedBuf")
            .field("len", &self.len())
            .field("align", &self.align())
            .finish_non_exhaustive()
    }
}

impl PartialEq for AlignedBuf {
    /// Compares contents and alignment, not addresses.
    fn eq(&self, other: &Self) -> bool {
        self.align() == other.align() && self.as_slice() == other.as_slice()
    }
}

impl Eq for AlignedBuf {}

// SAFETY: the buffer owns its allocation exclusively and holds plain bytes with no interior
// mutability, so moving it between threads is only a move of that ownership. The raw pointer is what
// blocks the automatic implementation, and it is an owning pointer, not a borrow.
unsafe impl Send for AlignedBuf {}

// SAFETY: as above; a shared reference gives only shared access to plain bytes.
unsafe impl Sync for AlignedBuf {}

// SAFETY: the bytes live in a heap allocation whose address is independent of where the value sits,
// so moving the value does not move them; the length is fixed at construction; every byte is
// initialised, zeroed or copied; and nothing here reallocates. A non-empty buffer is freed only
// when dropped. An empty one has a stable non-null aligned address and no bytes to keep.
unsafe impl IoBuf for AlignedBuf {
    fn stable_ptr(&self) -> *const u8 {
        self.pointer.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.layout.size()
    }
}

// SAFETY: a buffer uniquely owns its allocation, so `&mut self` is exclusive access to the bytes,
// and `stable_mut_ptr` returns the same address `stable_ptr` does.
unsafe impl IoBufMut for AlignedBuf {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.pointer.as_ptr()
    }
}

#[cfg(test)]
mod tests;
