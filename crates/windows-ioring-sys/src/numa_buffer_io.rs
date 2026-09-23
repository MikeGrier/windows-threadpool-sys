// Copyright (c) 2026 Mike Grier
//! This crate's I/O buffer traits, implemented for [`win_numa_sys::NumaBuffer`].
//!
//! # Why the impls are here and the type is not
//!
//! The allocator moved to `win-numa-sys`, which deliberately defines no buffer
//! trait: this crate and `windows-overlapped-io-sys` each already have their
//! own [`IoBuf`]/[`IoBufMut`] pair, duplicated under
//! [D-1](../DESIGN-NOTES.md#d-1)'s duplicate-then-decide with the merge still
//! open as `M6+.6`. A third copy in the allocator crate would have made that
//! decision harder rather than easier, and would have forced whichever traits
//! it chose onto every consumer of a NUMA buffer.
//!
//! The orphan rule permits the arrangement that avoids all of it. This crate
//! **owns** `IoBuf`, so it may implement it for a foreign type, and the
//! allocator stays trait-free with plain inherent accessors. `M6+.6` is
//! untouched: whatever it decides, these two impls are where this crate's
//! answer lands, and nothing in `win-numa-sys` has to move again.

use win_numa_sys::NumaBuffer;

use crate::{IoBuf, IoBufMut};

// SAFETY: the allocation's address is fixed once `VirtualAllocExNuma` returns
// it and does not move for the value's life; the requested length is fixed
// too. `NumaBuffer` is `Send`, which `IoBuf` requires.
unsafe impl IoBuf for NumaBuffer {
    fn stable_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.len()
    }
}

// SAFETY: a `NumaBuffer` uniquely owns its allocation, so `&mut self` is
// exclusive access to the bytes; the address is the same one `stable_ptr`
// reports, because both return the allocation's base.
unsafe impl IoBufMut for NumaBuffer {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }
}

#[cfg(test)]
mod tests;
