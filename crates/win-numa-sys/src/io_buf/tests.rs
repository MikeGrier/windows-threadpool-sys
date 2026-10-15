// Copyright (c) 2026 Mike Grier
// Copied from windows-ioring-sys src/numa_buffer_io/tests.rs at b6ba19e5.
//! Tests for the [`IoBuf`]/[`IoBufMut`] impls over [`NumaBuffer`].
//!
//! # What these are for
//!
//! The impls are two lines each and delegate to inherent accessors, so the risk
//! is not that the bodies are wrong -- it is that they are wired to the wrong
//! thing, and a delegation that returns a plausible pointer is exactly the kind
//! of defect that reads as correct. What is asserted is therefore the
//! *correspondence*: that the trait methods report the same address and the
//! same length the buffer itself does, and that the address is the one the
//! bytes actually live at.
//!
//! These also serve as the check that the arrangement holds at all: if the traits moved again, this
//! file is where that breaks first.

use io_buffers::{IoBuf, IoBufMut};

use crate::NumaBuffer;

/// The trait methods agree with the inherent ones.
#[test]
fn the_impls_report_what_the_buffer_reports() {
    let mut buffer = NumaBuffer::new(4096, None).expect("a valid allocation");
    let inherent_ptr = buffer.as_ptr();
    let inherent_len = buffer.len();

    assert_eq!(buffer.stable_ptr(), inherent_ptr);
    assert_eq!(buffer.bytes_len(), inherent_len);
    assert_eq!(buffer.stable_mut_ptr().cast_const(), inherent_ptr);
}

/// `bytes_len` is the requested length, which is what a kernel call is told.
///
/// The allocation is page-granular, so the mapping is larger than this for any
/// request under a page. Reporting the mapping's size instead would tell the
/// kernel it may use bytes the caller never asked about.
#[test]
fn bytes_len_is_the_requested_length_not_the_mapped_one() {
    for len in [1_usize, 100, 4095, 4096, 4097] {
        let buffer = NumaBuffer::new(len, None).expect("a valid allocation");
        assert_eq!(buffer.bytes_len(), len, "for a request of {len} bytes");
    }
}

/// The address the traits report is the address the bytes are at.
///
/// Writing through `stable_mut_ptr` and reading back through `stable_ptr` is
/// what proves the two are the same region rather than two plausible pointers.
#[test]
fn the_reported_address_is_where_the_bytes_are() {
    let mut buffer = NumaBuffer::new(4096, None).expect("a valid allocation");
    let len = buffer.bytes_len();

    // SAFETY: the pointer is this buffer's own allocation of at least `len`
    // bytes, and no other reference to it exists.
    unsafe { std::slice::from_raw_parts_mut(buffer.stable_mut_ptr(), len) }.fill(0xA5);

    // SAFETY: same region, now read-only, still exclusively owned here.
    let read = unsafe { std::slice::from_raw_parts(buffer.stable_ptr(), len) };
    assert!(read.iter().all(|&byte| byte == 0xA5));
}

/// The address does not change when the value moves.
///
/// `IoBuf`'s contract is that the pointer is stable for the value's life, and
/// a buffer handed to a ring is routinely moved into a collection first. The
/// allocation is owned by address rather than inline, so this holds -- but it
/// holds by construction rather than by accident, and the test is what says so.
#[test]
fn the_address_survives_a_move() {
    let buffer = NumaBuffer::new(4096, None).expect("a valid allocation");
    let before = buffer.stable_ptr();
    let moved = buffer;
    assert_eq!(moved.stable_ptr(), before);
}

/// A `NumaBuffer` is usable where the traits are the bound, which is how a ring takes a buffer: the
/// impls exist for the type, and a generic consumer needs nothing else of it.
#[test]
fn a_numa_buffer_satisfies_the_traits_as_a_bound() {
    fn length_through_the_bound<B: IoBufMut>(mut buffer: B) -> (usize, bool) {
        let shared = buffer.stable_ptr();
        (
            buffer.bytes_len(),
            buffer.stable_mut_ptr().cast_const() == shared,
        )
    }

    let buffer = NumaBuffer::new(1000, None).expect("a valid allocation");
    assert_eq!(length_through_the_bound(buffer), (1000, true));
}
