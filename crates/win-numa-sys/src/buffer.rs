// Copyright (c) 2026 Mike Grier
// Moved from windows-ioring-sys/src/numa_buffer.rs at 6101ca65, which had in
// turn moved it from examples/ring_copy/buffer.rs at 834c7afa.
//! A `VirtualAllocExNuma`-backed buffer, so a buffer can be placed on a chosen
//! NUMA node rather than wherever the default allocator's own heuristics land
//! it.
//!
//! # Why this is its own crate
//!
//! It was in `windows-ioring-sys`, which recommends placing a registered pool
//! near the device and so had a reason to provide the allocator rather than
//! leave every caller to write it. But the buffer has nothing to do with a
//! ring: its only connection was one doc line saying it *can* be registered
//! into one. Meanwhile `windows-placement-probe` -- which does not depend on
//! the ring crate -- had written the same `VirtualAllocExNuma` call for itself,
//! making two independent allocators in a workspace that had already hoisted
//! this code once to stop exactly that.
//!
//! # What this does not decide
//!
//! **Which node.** A caller who knows their storage or thread layout passes
//! `Some(node)`; one who does not passes `None` and gets the system's own
//! choice, which is what the default allocator would have given anyway.
//! [`volume_numa_node`](crate::volume_numa_node) is available for finding a
//! candidate, and says plainly what its answer is and is not worth.
//!
//! A node argument is a *preference*, which the underlying parameter says in
//! its own name (`nndPreferred`). So a successful allocation is not by itself
//! evidence that the pages landed on the node that was asked for, and code
//! that needs to know must measure rather than assume -- see the crate root.

use std::io;
use std::ptr;

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocExNuma, VirtualFree,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::NumaNode;

/// `VirtualAllocExNuma`'s documented sentinel for "no NUMA preference" --
/// windows-sys does not name this constant, so it is named here rather than
/// written as a bare literal at the call site.
const NUMA_NO_PREFERRED_NODE: u32 = u32::MAX;

/// An owned buffer allocated with `VirtualAllocExNuma`, freed with
/// `VirtualFree` on drop.
///
/// The allocation is page-granular: `VirtualAllocExNuma` rounds a request up
/// to the system page size, so a caller asking for a small buffer gets at
/// least a page. [`NumaBuffer::len`] reports the length that was *requested*,
/// which is the length a kernel call should be told about.
///
/// # Buffer traits live with whoever owns them
///
/// This type implements no I/O buffer trait, because this crate defines none
/// and should not: `windows-ioring-sys` and `windows-overlapped-io-sys` each
/// have their own `IoBuf`/`IoBufMut`, and a third copy here would be one more
/// of a thing the workspace is already deciding what to do about. A consumer
/// that owns such a trait implements it for this type -- the orphan rule
/// permits exactly that, since the trait is theirs -- over
/// [`NumaBuffer::as_ptr`], [`NumaBuffer::as_mut_ptr`] and [`NumaBuffer::len`].
/// `windows-ioring-sys` does so.
pub struct NumaBuffer {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the allocation is exclusively owned by this value; sending it
// across threads only moves that ownership, never aliases it.
unsafe impl Send for NumaBuffer {}

impl std::fmt::Debug for NumaBuffer {
    /// Shows the address and requested length, never the contents: a
    /// registered buffer routinely holds someone's data, and a `Debug` that
    /// printed it would put that data anywhere a caller logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NumaBuffer")
            .field("ptr", &self.ptr)
            .field("len", &self.len)
            .finish()
    }
}

impl NumaBuffer {
    /// Allocate `len` bytes, preferring `node` if given.
    ///
    /// Passing `None` requests no NUMA preference, which is the same choice
    /// the default allocator makes implicitly.
    ///
    /// # What a successful return does and does not mean
    ///
    /// A valid node is a **preference**, not an instruction -- `nndPreferred`
    /// says so in its name -- so success means the request was accepted and
    /// memory was obtained, never that the pages came from the node asked for.
    ///
    /// An *invalid* node is a different matter and is genuinely refused: this
    /// crate's tests assert that `u32::MAX - 1` fails. Note that `u32::MAX`
    /// itself is **not** a test of that, because it is the API's own
    /// no-preference sentinel and is accepted by design -- a measurement that
    /// reads it as an out-of-range node being tolerated has measured the
    /// sentinel instead.
    ///
    /// # Errors
    ///
    /// The error from `VirtualAllocExNuma`, which includes a `len` of zero and
    /// a node number the machine does not have.
    pub fn new(len: usize, node: Option<NumaNode>) -> io::Result<Self> {
        // SAFETY: no pointer arguments; the returned value is a pseudo-handle
        // that needs no closing.
        let process = unsafe { GetCurrentProcess() };
        // SAFETY: `process` is a valid pseudo-handle for the duration of this
        // call; a null `lpAddress` lets the system choose the address.
        let ptr = unsafe {
            VirtualAllocExNuma(
                process,
                ptr::null(),
                len,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
                node.map_or(NUMA_NO_PREFERRED_NODE, NumaNode::get),
            )
        };
        if ptr.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast(),
            len,
        })
    }

    /// The allocation's base address, fixed for this value's life.
    #[must_use]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// The allocation's base address for writing.
    ///
    /// Takes `&mut self` because this value uniquely owns the allocation, so
    /// exclusive access to the value is exclusive access to the bytes.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// The length that was requested.
    ///
    /// Not the length that was reserved: `VirtualAllocExNuma` rounds up to a
    /// page, so the mapping is at least this large and usually larger. This is
    /// the number a kernel call should be told, because it is the number the
    /// caller asked to use.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the requested length was zero.
    ///
    /// Present because clippy asks for it beside [`Self::len`], and because a
    /// zero-length request is rejected by `VirtualAllocExNuma` rather than
    /// producing an empty buffer -- so on any value that exists, this is false.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for NumaBuffer {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` was returned by `VirtualAllocExNuma` above and
        // is freed exactly once, here.
        unsafe {
            VirtualFree(self.ptr.cast(), 0, MEM_RELEASE);
        }
    }
}

#[cfg(test)]
mod tests;
