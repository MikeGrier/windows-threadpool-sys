// Copyright (c) 2026 Mike Grier
//! Unit tests for the owned-buffer traits.
//!
//! The property under test is the one the compiler cannot check and the one
//! this crate's soundness rests on: the address does not move.

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
    // Against an *independently* obtained address, not against another call to
    // `stable_ptr`. Comparing the function to itself passes even when it
    // returns null -- which is the address this crate hands the kernel -- and
    // mutation testing caught exactly that here (M18.3/M18.4).
    assert_eq!(
        buffer.stable_ptr(),
        Arc::as_ptr(&buffer).cast::<u8>(),
        "the reported address must be the allocation's own"
    );
    assert_eq!(
        buffer.stable_ptr(),
        clone.stable_ptr(),
        "clones must name the same bytes"
    );
}

#[test]
fn an_arc_slices_address_survives_moving_the_arc() {
    let buffer: Arc<[u8]> = Arc::from(vec![0_u8; 24].into_boxed_slice());
    // Independent of `stable_ptr`, so a constant return cannot satisfy this.
    let before = Arc::as_ptr(&buffer).cast::<u8>();
    assert_eq!(buffer.stable_ptr(), before);
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
fn a_static_mut_slice_is_readable_and_writable() {
    // The one reference type that is a legal read destination: `&'static mut`
    // is exclusive, so unlike `Arc<[u8]>` or `&'static [u8]` nothing else can
    // observe the kernel writing.
    let buffer: &'static mut [u8] = Box::leak(vec![0_u8; 32].into_boxed_slice());
    let before = buffer.stable_ptr();
    assert_eq!(buffer.bytes_len(), 32);
    let mut buffer = buffer;
    assert_eq!(buffer.stable_mut_ptr().cast_const(), before);
}

#[test]
fn a_static_mut_slices_address_survives_moving_the_reference() {
    let buffer: &'static mut [u8] = Box::leak(vec![0_u8; 16].into_boxed_slice());
    let before = buffer.stable_ptr();
    let moved = buffer;
    assert_eq!(
        moved.stable_ptr(),
        before,
        "the reference moved, the bytes must not"
    );
}

#[test]
fn an_empty_buffer_is_representable() {
    // A zero-length operation is legal, so an empty buffer must not be special.
    let buffer: Vec<u8> = Vec::new();
    assert_eq!(buffer.bytes_len(), 0);
}

#[test]
fn bytes_len_is_the_length_not_the_capacity() {
    let mut buffer = Vec::with_capacity(4096);
    buffer.extend_from_slice(&[1_u8, 2, 3]);
    assert_eq!(buffer.bytes_len(), 3);
    assert!(buffer.capacity() >= 4096);
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
