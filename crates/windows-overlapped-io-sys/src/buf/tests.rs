// Copyright (c) 2026 Mike Grier
//! Unit tests for the owned-buffer traits.
//!
//! The property under test is the one the compiler cannot check and the one an
//! operation's soundness rests on: the address does not move.

use std::sync::Arc;

use super::{IoBuf, IoBufMut};

#[test]
fn a_vec_reports_its_own_allocation_and_length() {
    let buffer = vec![1_u8, 2, 3, 4];
    assert_eq!(buffer.stable_ptr(), buffer.as_ptr());
    assert_eq!(buffer.bytes_len(), 4);
}

#[test]
fn a_vecs_address_survives_moving_the_vec() {
    // The property every impl exists to promise: the bytes are in a heap
    // allocation, so moving the handle to them does not move them. An inline
    // array would fail this, which is why none implements the trait.
    let buffer = vec![0_u8; 32];
    let before = buffer.stable_ptr();
    let moved = buffer;
    assert_eq!(
        moved.stable_ptr(),
        before,
        "the buffer moved out from under us"
    );
}

#[test]
fn a_vecs_mutable_address_matches_its_shared_one() {
    // Required by `IoBufMut`: the kernel is handed one address, and a caller
    // reading the result later looks at the other.
    let mut buffer = vec![0_u8; 16];
    let shared = buffer.stable_ptr();
    assert_eq!(buffer.stable_mut_ptr().cast_const(), shared);
}

#[test]
fn a_boxed_slice_is_readable_and_writable() {
    let mut buffer: Box<[u8]> = vec![7_u8; 8].into_boxed_slice();
    assert_eq!(buffer.bytes_len(), 8);
    let before = buffer.stable_ptr();
    assert_eq!(buffer.stable_mut_ptr().cast_const(), before);
}

#[test]
fn a_boxed_slices_address_survives_moving_the_box() {
    let buffer: Box<[u8]> = vec![0_u8; 64].into_boxed_slice();
    let before = buffer.stable_ptr();
    let moved = buffer;
    assert_eq!(moved.stable_ptr(), before);
}

#[test]
fn an_arc_slice_is_readable_and_shares_one_allocation() {
    // The case that motivates splitting the traits: an `Arc<[u8]>` is a fine
    // source and can never be a destination, because its clones alias.
    let buffer: Arc<[u8]> = Arc::from(vec![9_u8; 12].into_boxed_slice());
    let clone = Arc::clone(&buffer);
    assert_eq!(buffer.bytes_len(), 12);
    assert_eq!(
        buffer.stable_ptr(),
        clone.stable_ptr(),
        "clones must name the same bytes"
    );
}

#[test]
fn an_arc_slices_address_survives_moving_the_arc() {
    let buffer: Arc<[u8]> = Arc::from(vec![0_u8; 24].into_boxed_slice());
    let before = buffer.stable_ptr();
    let moved = buffer;
    assert_eq!(moved.stable_ptr(), before);
}

#[test]
fn a_static_slice_is_readable() {
    const DATA: &[u8] = b"static payload";
    let buffer: &'static [u8] = DATA;
    assert_eq!(buffer.bytes_len(), DATA.len());
    assert_eq!(buffer.stable_ptr(), DATA.as_ptr());
}

#[test]
fn an_empty_buffer_is_representable() {
    // A zero-length operation is legal, so an empty buffer must not be special.
    let buffer: Vec<u8> = Vec::new();
    assert_eq!(buffer.bytes_len(), 0);
}

#[test]
fn bytes_len_is_the_length_not_the_capacity() {
    // The operation may only touch what is initialized. Reporting capacity would
    // hand the kernel uninitialized bytes to read from on a write.
    let mut buffer = Vec::with_capacity(4096);
    buffer.extend_from_slice(&[1_u8, 2, 3]);
    assert_eq!(buffer.bytes_len(), 3);
    assert!(buffer.capacity() >= 4096);
}

#[test]
fn page_buffers_are_readable_and_writable() {
    use crate::PageBuffers;

    let mut buffers = PageBuffers::new(2);
    assert_eq!(buffers.bytes_len(), buffers.len());
    let before = buffers.stable_ptr();
    assert_eq!(buffers.stable_mut_ptr().cast_const(), before);
    assert_eq!(
        before.addr() % crate::PAGE_SIZE,
        0,
        "the alignment a caller chose PageBuffers for must survive the trait"
    );
}

#[test]
fn page_buffers_addresses_survive_moving_the_value() {
    use crate::PageBuffers;

    let buffers = PageBuffers::new(1);
    let before = buffers.stable_ptr();
    let moved = buffers;
    assert_eq!(moved.stable_ptr(), before);
}

/// A caller-defined buffer, standing in for a pooled or alignment-constrained
/// one: the extension point the traits exist to offer.
struct CustomBuffer {
    inner: Box<[u8]>,
}

// SAFETY: the bytes live in an owned boxed slice, so the address is stable
// across moves and the length is fixed.
unsafe impl IoBuf for CustomBuffer {
    fn stable_ptr(&self) -> *const u8 {
        self.inner.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.inner.len()
    }
}

// SAFETY: unique ownership, same allocation as `stable_ptr`.
unsafe impl IoBufMut for CustomBuffer {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.inner.as_mut_ptr()
    }
}

#[test]
fn a_caller_defined_buffer_can_implement_the_traits() {
    let mut buffer = CustomBuffer {
        inner: vec![0_u8; 128].into_boxed_slice(),
    };
    assert_eq!(buffer.bytes_len(), 128);
    let before = buffer.stable_ptr();
    assert_eq!(buffer.stable_mut_ptr().cast_const(), before);
    let moved = buffer;
    assert_eq!(moved.stable_ptr(), before);
}
