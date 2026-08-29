// Copyright (c) 2026 Mike Grier
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLUSH_DATA, FILE_FLUSH_DEFAULT, FILE_FLUSH_MIN_METADATA, FILE_FLUSH_NO_SYNC,
    FILE_WRITE_FLAGS_NONE, FILE_WRITE_FLAGS_WRITE_THROUGH, IOSQE_FLAGS_DRAIN_PRECEDING_OPS,
    IOSQE_FLAGS_NONE,
};

use super::{Batch, FlushCoverage, FlushMode, PushOptions, WriteCaching};
use crate::IoRing;
use crate::buf::{IoBuf, IoBufMut};

#[test]
fn default_push_options_set_no_barrier() {
    assert_eq!(PushOptions::new().sqe_flags(), IOSQE_FLAGS_NONE);
}

#[test]
fn a_covering_flush_sets_the_barrier_flag_and_an_unordered_one_does_not() {
    // The mapping this whole type exists for (M12.1, D-23): a covering flush
    // must carry the barrier, because without it the flush's completion says
    // nothing about the writes before it. Pinned here so a future edit to
    // `sqe_flags` cannot silently turn every covering flush into an unordered
    // one -- which would compile, pass every other test, and lose data only
    // on power failure.
    assert_eq!(
        FlushCoverage::CoversPrecedingOperations.sqe_flags(),
        IOSQE_FLAGS_DRAIN_PRECEDING_OPS
    );
    assert_eq!(FlushCoverage::Unordered.sqe_flags(), IOSQE_FLAGS_NONE);
}

#[test]
fn write_caching_maps_to_the_kernels_write_flags() {
    // `Cached` is the default and must stay the no-flag value: silently
    // enabling write-through would change latency behaviour for every existing
    // caller without changing any call site (M12.3).
    assert_eq!(WriteCaching::Cached.raw(), FILE_WRITE_FLAGS_NONE);
    assert_eq!(
        WriteCaching::WriteThrough.raw(),
        FILE_WRITE_FLAGS_WRITE_THROUGH
    );
    assert_eq!(WriteCaching::default(), WriteCaching::Cached);
}

#[test]
fn flush_mode_maps_to_the_kernels_flush_modes() {
    // The mapping matters most for `NoSync`, which is the one mode that issues
    // no device sync: mixing it up with any other value would turn a commit
    // point into a no-op that still reports success (M12.4).
    assert_eq!(FlushMode::Default.raw(), FILE_FLUSH_DEFAULT);
    assert_eq!(FlushMode::Data.raw(), FILE_FLUSH_DATA);
    assert_eq!(FlushMode::MinMetadata.raw(), FILE_FLUSH_MIN_METADATA);
    assert_eq!(FlushMode::NoSync.raw(), FILE_FLUSH_NO_SYNC);
    assert_eq!(FlushMode::default(), FlushMode::Default);
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
    let error = unsafe { batch.read_raw(NULL_FILE, HugeBuffer, 0, PushOptions::new()) }
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
    let error = unsafe {
        batch.write_raw(
            NULL_FILE,
            HugeBuffer,
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
    }
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
