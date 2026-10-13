// Copyright (c) 2026 Mike Grier
//! One buffer type, implemented once, is accepted by both ring crates (IB-2+.1).
//!
//! `windows-ioring-sys` and `windows-overlapped-io-sys` each used to define their own `IoBuf` and
//! `IoBufMut`, so a buffer type used with both implemented two traits. They now re-export
//! `io-buffers`', so one implementation serves both. The claim is about a trait being the same
//! trait, which is a claim for the compiler: if either crate defined its own again, the generic
//! functions below would fail to **compile** for a type that implements only `io-buffers`', which
//! is a stronger rung than any assertion.

use io_buffers::{AlignedBuf, IoBufMut};

/// A caller-defined buffer that implements `io-buffers`' traits and no other crate's.
struct Pooled(Box<[u8]>);

// SAFETY: the bytes live in an owned boxed slice, so the address is stable across moves, the
// length is fixed, and nothing else holds them.
unsafe impl io_buffers::IoBuf for Pooled {
    fn stable_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.0.len()
    }
}

// SAFETY: unique ownership, and the same allocation `stable_ptr` reports.
unsafe impl IoBufMut for Pooled {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr()
    }
}

fn through_the_ring_crate<B: windows_ioring_sys::IoBufMut>(mut buffer: B) -> (usize, *const u8) {
    use windows_ioring_sys::IoBuf;
    let shared = buffer.stable_ptr();
    assert_eq!(buffer.stable_mut_ptr().cast_const(), shared);
    (buffer.bytes_len(), shared)
}

fn through_the_overlapped_crate<B: windows_overlapped_io_sys::IoBufMut>(
    mut buffer: B,
) -> (usize, *const u8) {
    use windows_overlapped_io_sys::IoBuf;
    let shared = buffer.stable_ptr();
    assert_eq!(buffer.stable_mut_ptr().cast_const(), shared);
    (buffer.bytes_len(), shared)
}

#[test]
fn a_type_implementing_only_the_shared_traits_satisfies_both_crates_bounds() {
    let first = Pooled(vec![0_u8; 64].into_boxed_slice());
    let address = io_buffers::IoBuf::stable_ptr(&first);
    assert_eq!(through_the_ring_crate(first), (64, address));

    let second = Pooled(vec![0_u8; 128].into_boxed_slice());
    let address = io_buffers::IoBuf::stable_ptr(&second);
    assert_eq!(through_the_overlapped_crate(second), (128, address));
}

#[test]
fn the_shared_aligned_buffer_is_accepted_by_both_crates() {
    let ring = AlignedBuf::zeroed(4096, 4096).expect("a buffer");
    let ring_address = io_buffers::IoBuf::stable_ptr(&ring);
    assert_eq!(through_the_ring_crate(ring), (4096, ring_address));

    let overlapped = AlignedBuf::zeroed(512, 512).expect("a buffer");
    let overlapped_address = io_buffers::IoBuf::stable_ptr(&overlapped);
    assert_eq!(
        through_the_overlapped_crate(overlapped),
        (512, overlapped_address)
    );
}

#[test]
fn the_two_crates_name_the_same_trait_for_the_standard_types_too() {
    assert_eq!(through_the_ring_crate(vec![1_u8; 10]).0, 10);
    assert_eq!(through_the_overlapped_crate(vec![1_u8; 10]).0, 10);
}
