// Copyright (c) 2026 Mike Grier
// Copied from windows-ioring-sys src/numa_buffer_io.rs at ea5d17d1.
//! `io-buffers`' I/O buffer traits, implemented for [`NumaBuffer`].
//!
//! # Why the impls are here
//!
//! A trait may be implemented for a type only by the crate that owns the trait or the crate that
//! owns the type. `io-buffers` owns the traits and this crate owns the type, and `io-buffers` is
//! portable and cannot depend on a Windows allocator, so the impls are here. They are two lines each
//! and delegate to [`NumaBuffer`]'s inherent accessors, which are public, so a crate that owns a
//! buffer trait of its own can implement it the same way.

use io_buffers::{IoBuf, IoBufMut};

use crate::NumaBuffer;

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
