// Copyright (c) 2026 Mike Grier
//! A test buffer that reports a length no `u32` can express.

use io_buffers::{IoBuf, IoBufMut};

/// A buffer that reports a length no `u32` can express, without allocating one.
///
/// Every adapter checks the length and rejects the request before issuing any
/// native call, so this value is never owned by an operation and its pointer is
/// never dereferenced -- which is the only reason it can exist. The traits'
/// validity promise binds while an operation holds the buffer, and no operation
/// ever holds this one.
#[derive(Debug)]
pub(crate) struct OversizedBuffer;

// SAFETY: see the type's documentation -- the promise is never relied upon,
// because every path that would rely on it rejects this length first.
unsafe impl IoBuf for OversizedBuffer {
    fn stable_ptr(&self) -> *const u8 {
        std::ptr::NonNull::<u8>::dangling().as_ptr()
    }

    fn bytes_len(&self) -> usize {
        u32::MAX as usize + 1
    }
}

// SAFETY: as above.
unsafe impl IoBufMut for OversizedBuffer {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        std::ptr::NonNull::<u8>::dangling().as_ptr()
    }
}
