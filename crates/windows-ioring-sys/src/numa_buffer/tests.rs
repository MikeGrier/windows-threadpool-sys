// Copyright (c) 2026 Mike Grier
//! Tests for [`NumaBuffer`] (M22.3).
//!
//! These allocate through `VirtualAllocExNuma` and free on drop. That is an
//! operating-system call, but this repository targets one operating system, so
//! by its own Quality rule that alone does not make these integration tests:
//! they cross no process, device, or network boundary, hold no handle, and
//! each completes in microseconds.
//!
//! # What these cannot establish
//!
//! **That the pages landed on the requested node.** The parameter is
//! `nndPreferred`, so a success proves the request was accepted, not that it
//! was honoured -- and a single-node host, which is what this is developed on,
//! could not tell the difference either way. The type's own documentation says
//! this; these tests do not pretend otherwise by asserting a node back.

use super::NumaBuffer;
use crate::{IoBuf, IoBufMut};

/// A node number no machine has, for the rejection cases. `u32::MAX` is not
/// usable here -- it is `VirtualAllocExNuma`'s own "no preference" sentinel,
/// so it would be accepted rather than refused.
const ABSURD_NODE: u32 = u32::MAX - 1;

#[test]
fn an_unplaced_buffer_allocates() {
    let buffer = NumaBuffer::new(4096, None).expect("an unplaced allocation");
    assert!(!buffer.stable_ptr().is_null());
}

#[test]
fn node_zero_allocates() {
    // Every machine reports a node 0, including one with NUMA disabled, where
    // `GetNumaHighestNodeNumber` answers 0.
    let buffer = NumaBuffer::new(4096, Some(0)).expect("node 0 exists everywhere");
    assert!(!buffer.stable_ptr().is_null());
}

#[test]
fn the_reported_length_is_the_requested_one_not_the_rounded_one() {
    // The allocation is page-granular, so the *mapping* is at least a page.
    // What the kernel is told about an operation is `bytes_len`, and that must
    // be what the caller asked for -- reporting the rounded-up size would
    // invite a read or write past the caller's intent.
    for len in [1_usize, 100, 4095, 4096, 4097, 65536] {
        let buffer = NumaBuffer::new(len, None).expect("a valid allocation");
        assert_eq!(buffer.bytes_len(), len, "for a request of {len} bytes");
    }
}

#[test]
fn a_fresh_allocation_is_zeroed() {
    // `VirtualAlloc`-family pages arrive zeroed, which is what lets a caller
    // register an arena without filling it first.
    let mut buffer = NumaBuffer::new(8192, None).expect("a valid allocation");
    // SAFETY: the pointer is this buffer's own allocation of `bytes_len`
    // bytes, and `&mut` makes the borrow exclusive.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.stable_mut_ptr(), buffer.bytes_len()) };
    assert!(bytes.iter().all(|&b| b == 0));
}

#[test]
fn bytes_written_read_back() {
    let mut buffer = NumaBuffer::new(4096, None).expect("a valid allocation");
    let len = buffer.bytes_len();
    // SAFETY: as above -- this buffer's own allocation, exclusively borrowed.
    let bytes = unsafe { std::slice::from_raw_parts_mut(buffer.stable_mut_ptr(), len) };
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    // SAFETY: as above.
    let read = unsafe { std::slice::from_raw_parts(buffer.stable_ptr(), len) };
    assert!(read.iter().enumerate().all(|(i, &b)| b == (i % 251) as u8));
}

#[test]
fn the_address_is_stable_across_reads() {
    // The whole reason this type can be registered: the address it reports
    // does not move for its life.
    let mut buffer = NumaBuffer::new(4096, None).expect("a valid allocation");
    let first = buffer.stable_ptr();
    assert_eq!(buffer.stable_ptr(), first);
    assert_eq!(buffer.stable_mut_ptr().cast_const(), first);
    assert_eq!(buffer.stable_ptr(), first);
}

#[test]
fn an_address_survives_a_move() {
    // `IoBuf`'s contract is that the address is stable for the value's life,
    // which includes being moved -- the allocation is behind a pointer, so
    // moving the handle does not move the bytes.
    let buffer = NumaBuffer::new(4096, None).expect("a valid allocation");
    let before = buffer.stable_ptr();
    let moved = buffer;
    assert_eq!(moved.stable_ptr(), before);
}

#[test]
fn separate_buffers_do_not_alias() {
    let buffers: Vec<NumaBuffer> = (0..8)
        .map(|_| NumaBuffer::new(4096, None).expect("a valid allocation"))
        .collect();
    let mut addresses: Vec<*const u8> = buffers.iter().map(IoBuf::stable_ptr).collect();
    addresses.sort_unstable();
    let before = addresses.len();
    addresses.dedup();
    assert_eq!(addresses.len(), before, "every slot must be its own memory");
}

#[test]
fn a_zero_length_request_is_refused() {
    let error = NumaBuffer::new(0, None).expect_err("a zero-length mapping is not allocatable");
    assert!(error.raw_os_error().is_some(), "and it is an OS error");
}

#[test]
fn a_node_the_machine_does_not_have_is_refused() {
    // The other half of the guard: the accepting cases above would all still
    // pass if `new` ignored its node argument entirely.
    let error =
        NumaBuffer::new(4096, Some(ABSURD_NODE)).expect_err("no machine has this node number");
    assert!(error.raw_os_error().is_some(), "and it is an OS error");
}

#[test]
fn a_request_the_address_space_cannot_hold_is_refused() {
    let error = NumaBuffer::new(usize::MAX, None).expect_err("no process has this much space");
    assert!(error.raw_os_error().is_some(), "and it is an OS error");
}

#[test]
fn a_refused_allocation_leaves_the_allocator_usable() {
    // A failed `VirtualAllocExNuma` must not leave anything behind that stops
    // the next one -- the sample arenas allocate in a loop, so one bad request
    // in the middle would otherwise take the rest with it.
    let _ = NumaBuffer::new(4096, Some(ABSURD_NODE)).expect_err("refused");
    let buffer = NumaBuffer::new(4096, None).expect("the next allocation still works");
    assert!(!buffer.stable_ptr().is_null());
}

#[test]
fn many_buffers_allocate_and_free() {
    // Drop is what returns the mapping; leaking it would show up here as an
    // address-space exhaustion long before the loop ends.
    for _ in 0..512 {
        let buffer = NumaBuffer::new(65536, None).expect("a valid allocation");
        assert!(!buffer.stable_ptr().is_null());
    }
}

#[test]
fn a_buffer_is_send() {
    // The `unsafe impl Send` is load-bearing: a domain runtime allocates on
    // one thread and uses the buffer on the pinned thread that owns the ring.
    fn assert_send<T: Send>() {}
    assert_send::<NumaBuffer>();

    let buffer = NumaBuffer::new(4096, None).expect("a valid allocation");
    let address = buffer.stable_ptr() as usize;
    let moved = std::thread::spawn(move || buffer.stable_ptr() as usize)
        .join()
        .expect("the thread completes");
    assert_eq!(moved, address, "and the address travels with it");
}
