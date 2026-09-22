// Copyright (c) 2026 Mike Grier
// Moved from examples/ring_copy/buffer.rs at 834c7afa. The move plus its
// library-quality documentation left the files 47% similar, which is under
// git's rename threshold, so blame and log do not follow it without help.
//! A `VirtualAllocExNuma`-backed buffer, so a registered buffer can be placed
//! on a chosen NUMA node rather than wherever the default allocator's own
//! heuristics land it.
//!
//! # Why this is in the library
//!
//! This crate's front page tells a caller that placing the registered pool on
//! the node closest to the device "is very likely the highest-leverage
//! locality decision available". Until M22.3 it then left every caller to
//! write the allocator themselves, and two in-repo consumers duly did --
//! `examples/ring_copy` first, `examples/epoch_log` second. Recommending an
//! allocation and not providing it invites exactly the duplicate that
//! recommendation is worth avoiding, so the allocator lives here and the
//! samples bind to it.
//!
//! # What this does not decide
//!
//! **Which node.** That answer depends on where the device is, and this crate
//! deliberately does not map a file to a node -- see "What is not reachable"
//! in [DESIGN-NOTES.md](../DESIGN-NOTES.md) for why
//! `FSCTL_QUERY_VOLUME_NUMA_INFO` answers a *volume*-level question that a
//! spanned volume or a Storage Spaces set can make meaningless. A caller who
//! knows their storage layout passes `Some(node)`; one who does not passes
//! `None` and gets the system's own choice, which is what the default
//! allocator would have given anyway.
//!
//! A node argument is a *preference*, which the underlying parameter says in
//! its own name (`nndPreferred`). So a successful allocation is not by itself
//! evidence that the pages landed on the node that was asked for, and code
//! that needs to know must measure rather than assume.

use std::io;
use std::ptr;

use crate::{IoBuf, IoBufMut};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocExNuma, VirtualFree,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// `VirtualAllocExNuma`'s documented sentinel for "no NUMA preference" --
/// windows-sys does not name this constant, so it is named here rather than
/// written as a bare literal at the call site.
const NUMA_NO_PREFERRED_NODE: u32 = u32::MAX;

/// An owned buffer allocated with `VirtualAllocExNuma`, freed with
/// `VirtualFree` on drop.
///
/// Implements [`IoBuf`] and [`IoBufMut`], so it can be handed to an operation
/// directly or registered into a ring with `Batch::register_buffers`.
///
/// The allocation is page-granular: `VirtualAllocExNuma` rounds a request up
/// to the system page size, so a caller asking for a small buffer gets at
/// least a page. [`IoBuf::bytes_len`] reports the length that was *requested*,
/// which is the length the kernel is told about.
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
    /// # Errors
    ///
    /// The error from `VirtualAllocExNuma`, which includes a `len` of zero
    /// and a `node` number the machine does not have.
    pub fn new(len: usize, node: Option<u32>) -> io::Result<Self> {
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
                node.unwrap_or(NUMA_NO_PREFERRED_NODE),
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

// SAFETY: the allocation's address is fixed once `VirtualAllocExNuma`
// returns it and does not move for this value's life; `len` is fixed too.
unsafe impl IoBuf for NumaBuffer {
    fn stable_ptr(&self) -> *const u8 {
        self.ptr
    }

    fn bytes_len(&self) -> usize {
        self.len
    }
}

// SAFETY: this value uniquely owns the allocation, so `&mut self` is
// exclusive access; the address is the same one `stable_ptr` reports.
unsafe impl IoBufMut for NumaBuffer {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }
}

#[cfg(test)]
mod tests;
