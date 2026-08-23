// Copyright (c) 2026 Mike Grier
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{IOSQE_FLAGS_DRAIN_PRECEDING_OPS, IOSQE_FLAGS_NONE};

use super::{Batch, PushOptions};
use crate::IoRing;
use crate::buf::{IoBuf, IoBufMut};

#[test]
fn default_push_options_set_no_barrier() {
    assert_eq!(PushOptions::new().sqe_flags(), IOSQE_FLAGS_NONE);
}

#[test]
fn drain_preceding_sets_the_barrier_flag() {
    assert_eq!(
        PushOptions::new().drain_preceding(true).sqe_flags(),
        IOSQE_FLAGS_DRAIN_PRECEDING_OPS
    );
    assert_eq!(
        PushOptions::new()
            .drain_preceding(true)
            .drain_preceding(false)
            .sqe_flags(),
        IOSQE_FLAGS_NONE
    );
}

/// A buffer that claims a length no real allocation could ever have, to
/// exercise `checked_len`'s rejection without needing a real file: the
/// rejection must happen before the buffer's pointer is ever read.
struct HugeBuffer;

// SAFETY: `stable_ptr`/`stable_mut_ptr` are never dereferenced in the tests
// that use this type -- `checked_len` rejects the operation first.
unsafe impl IoBuf for HugeBuffer {
    fn stable_ptr(&self) -> *const u8 {
        std::ptr::NonNull::dangling().as_ptr()
    }

    fn bytes_len(&self) -> usize {
        usize::MAX
    }
}

// SAFETY: see the `IoBuf` impl above.
unsafe impl IoBufMut for HugeBuffer {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        std::ptr::NonNull::dangling().as_ptr()
    }
}

const NULL_FILE: HANDLE = std::ptr::null_mut();

#[test]
fn read_rejects_a_buffer_longer_than_u32_max_without_touching_the_ring() {
    let mut ring = IoRing::new(8, 8).expect("create ring");
    let outstanding_before = ring.outstanding();
    let mut batch = Batch::new(&mut ring);
    // SAFETY: NULL_FILE is never dereferenced -- the oversized buffer is
    // rejected before the handle would be used.
    let error = unsafe { batch.read(NULL_FILE, HugeBuffer, 0, PushOptions::new()) }
        .expect_err("an oversized buffer must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    drop(batch);
    assert_eq!(
        ring.outstanding(),
        outstanding_before,
        "a rejected push must not reserve an identity"
    );
}

#[test]
fn write_rejects_a_buffer_longer_than_u32_max_without_touching_the_ring() {
    let mut ring = IoRing::new(8, 8).expect("create ring");
    let outstanding_before = ring.outstanding();
    let mut batch = Batch::new(&mut ring);
    // SAFETY: as above.
    let error = unsafe { batch.write(NULL_FILE, HugeBuffer, 0, PushOptions::new()) }
        .expect_err("an oversized buffer must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    drop(batch);
    assert_eq!(
        ring.outstanding(),
        outstanding_before,
        "a rejected push must not reserve an identity"
    );
}

#[test]
fn dropping_a_batch_that_queued_nothing_submits_harmlessly() {
    let mut ring = IoRing::new(8, 8).expect("create ring");
    let batch = Batch::new(&mut ring);
    drop(batch);
}
