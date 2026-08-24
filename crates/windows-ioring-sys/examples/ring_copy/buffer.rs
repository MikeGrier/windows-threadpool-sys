// Copyright (c) 2026 Mike Grier
//! A `VirtualAllocExNuma`-backed buffer (M7.3), so a domain's registered
//! buffer can be placed on a chosen NUMA node rather than wherever the
//! default allocator's own heuristics land it.

use std::io;
use std::ptr;

use windows_ioring_sys::{IoBuf, IoBufMut};
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
pub struct NumaBuffer {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the allocation is exclusively owned by this value; sending it
// across threads only moves that ownership, never aliases it.
unsafe impl Send for NumaBuffer {}

impl NumaBuffer {
    /// Allocate `len` bytes, preferring `node` if given.
    ///
    /// # Errors
    ///
    /// Returns the error from `VirtualAllocExNuma`.
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
