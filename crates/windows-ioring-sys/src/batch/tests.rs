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

// --- registration accessors and claim guards (M18.4) -------------------------
//
// Everything below closes a mutation-testing survivor from M18.3. These are
// unit tests rather than integration ones because the interesting cases -- an
// index past the end, a completion naming a different operation -- are cheap to
// construct directly and awkward to provoke through a real ring.

use super::{PendingFileRegistration, RegisteredFiles};
use crate::ring::Completion;

/// A registration whose base index is deliberately non-zero.
///
/// Every existing test registers one handle and reads index 0, where
/// `base_index + i` is indistinguishable from `base_index`, `base_index - i`
/// and `base_index * i`. All three arithmetic mutants survived on that.
fn registered_files(ring: &IoRing) -> RegisteredFiles {
    RegisteredFiles {
        base_index: 3,
        count: 2,
        ring_id: ring.ring_id(),
    }
}

#[test]
fn registered_files_report_their_extent() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let files = registered_files(&ring);

    assert_eq!(files.len(), 2);
    assert!(!files.is_empty());

    let empty = RegisteredFiles {
        base_index: 0,
        count: 0,
        ring_id: ring.ring_id(),
    };
    assert_eq!(empty.len(), 0);
    assert!(empty.is_empty());
}

#[test]
fn registered_files_index_from_the_base_and_stop_at_the_end() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let files = registered_files(&ring);

    assert_eq!(files.get(0).expect("index 0 is in range").index(), 3);
    assert_eq!(files.get(1).expect("index 1 is in range").index(), 4);
    assert!(
        files.get(2).is_none(),
        "index 2 is one past the end of a two-handle registration"
    );
    assert!(files.get(u32::MAX).is_none());
}

#[test]
fn a_pending_file_registration_claims_only_its_own_completion() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let other = IoRing::new(8, 8).expect("create second ring");
    let user_data = 41_usize;

    let pending = || PendingFileRegistration {
        user_data,
        base_index: 0,
        count: 1,
        ring_id: ring.ring_id(),
    };

    // Right ring, wrong operation.
    let wrong_op = Completion::synthetic(user_data.wrapping_add(1), 0, ring.ring_id());
    assert!(
        pending().claim_if(&wrong_op).is_err(),
        "a completion naming another operation must be refused"
    );

    // Right operation, wrong ring -- the half a `||`-to-`&&` mutant hides in.
    let wrong_ring = Completion::synthetic(user_data, 0, other.ring_id());
    assert!(
        pending().claim_if(&wrong_ring).is_err(),
        "a completion from another ring must be refused even with a matching id"
    );

    let matching = Completion::synthetic(user_data, 0, ring.ring_id());
    let files = pending()
        .claim_if(&matching)
        .expect("its own completion is accepted")
        .expect("the registration succeeded");
    assert_eq!(files.len(), 1);
}

#[test]
fn a_pending_file_registration_reports_the_user_data_it_will_claim() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let pending = PendingFileRegistration {
        user_data: 7,
        base_index: 0,
        count: 1,
        ring_id: ring.ring_id(),
    };
    assert_eq!(pending.user_data(), 7);
    // Claiming against exactly what was reported must work, which is what ties
    // the accessor to the guard rather than leaving it a free-floating number.
    let completion = Completion::synthetic(pending.user_data(), 0, ring.ring_id());
    assert!(pending.claim_if(&completion).is_ok());
}

#[test]
fn a_pending_buffer_registration_claims_only_its_own_completion() {
    let mut ring = IoRing::new(8, 8).expect("create ring");
    let other = IoRing::new(8, 8).expect("create second ring");

    // Burn the first two `UserData` values so the registration below is neither
    // operation 0 nor operation 1. A fresh ring starts at zero, so burning none
    // leaves `user_data() -> 0` indistinguishable from the truth, and burning
    // one leaves `-> 1` indistinguishable -- both of which survived in turn
    // while this test was being written.
    for expected in 0..2 {
        let burned = crate::Token::new(&mut ring, vec![0_u8; 1]).expect("mint a token");
        assert_eq!(
            burned.id(),
            expected,
            "a fresh ring hands out UserData from zero, in order"
        );
        drop(burned);
        ring.record_completion();
    }

    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_buffers(vec![vec![0_u8; 64]])
        .expect("queue buffer registration");
    assert_ne!(
        pending.user_data(),
        0,
        "the registration is not operation zero, so a constant zero cannot pass"
    );
    let user_data = pending.user_data();
    batch.submit_and_wait(1, 5_000).expect("submit");

    // Matched rather than `expect_err`, because `RegisteredBuffers` is not
    // `Debug` -- it owns caller buffers whose type need not be.
    let wrong_ring = Completion::synthetic(user_data, 0, other.ring_id());
    let Err(pending) = pending.claim_if(&wrong_ring) else {
        panic!("a completion from another ring must be refused");
    };

    let wrong_op = Completion::synthetic(user_data.wrapping_add(1), 0, ring.ring_id());
    let Err(pending) = pending.claim_if(&wrong_op) else {
        panic!("a completion naming another operation must be refused");
    };

    let real = ring
        .try_pop()
        .expect("pop")
        .expect("the registration completion is ready");
    // Ties the accessor to the operation it names, against an id obtained from
    // the ring rather than from the accessor itself -- a constant `user_data`
    // survives any comparison that starts from `user_data`.
    assert_eq!(
        pending.user_data(),
        real.user_data(),
        "the reported user_data must be the one the ring completed"
    );
    let mut buffers = pending
        .claim_if(&real)
        .expect("its own completion is accepted")
        .expect("the registration succeeded");

    assert_eq!(buffers.len(), 1);
    assert!(!buffers.is_empty());
    // `get` runs the bounds check `checked_index` owns; one past the end must
    // be refused rather than reaching into the registration's neighbour.
    assert!(buffers.get(0).is_ok());
    assert!(
        buffers.get(1).is_err(),
        "index 1 is one past the end of a one-buffer registration"
    );

    // `get` has its own bounds check; `checked_index` is the one the *span*
    // paths use, and it is only reachable by submitting against an index.
    let mut batch = Batch::new(&mut ring);
    let out_of_range = unsafe {
        batch.read_registered_raw(
            std::ptr::null_mut(),
            &buffers,
            crate::RegisteredSpan {
                buffer_index: 1,
                offset: 0,
                len: 8,
            },
            0,
            PushOptions::new(),
        )
    };
    assert!(
        out_of_range.is_err(),
        "a span naming one past the last registered buffer must be refused"
    );
}

#[test]
fn submit_reports_how_many_operations_it_queued() {
    // `Batch::submit -> Ok(0)` and `-> Ok(1)` both survived M18.3: every caller
    // discarded the count, so nothing observed it.
    let path = std::env::temp_dir().join(format!("ioring-submit-count-{}.tmp", std::process::id()));
    std::fs::write(&path, vec![1_u8; 4096]).expect("write fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open fixture");
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&file);

    let mut ring = IoRing::new(16, 16).expect("create ring");
    let mut tokens = Vec::new();
    let mut batch = Batch::new(&mut ring);
    for index in 0..3_u64 {
        // SAFETY: `file` outlives every operation -- all three are drained
        // before this test returns.
        let token =
            unsafe { batch.read_raw(handle, vec![0_u8; 512], index * 512, PushOptions::new()) }
                .expect("queue read");
        tokens.push(token);
    }
    assert_eq!(
        batch.submit().expect("submit"),
        3,
        "submit must report the number of operations it queued"
    );

    // Drain so the ring can be dropped with nothing outstanding.
    let mut popped = 0;
    while popped < 3 {
        while let Some(completion) = ring.try_pop().expect("pop") {
            let _ = completion.result();
            if let Some(position) = tokens.iter().position(|t| t.id() == completion.user_data()) {
                let token = tokens.swap_remove(position);
                let _ = token.claim_if(&completion);
            }
            popped += 1;
        }
    }

    drop(file);
    let _ = std::fs::remove_file(&path);
}

#[test]
#[should_panic(expected = "RegisteredBuffers dropped while an operation still references it")]
fn dropping_a_registration_with_work_outstanding_is_refused() {
    // M5.3's drop guard: freeing the backing memory while an
    // `IORING_BUFFER_REF` still points at it is the use-after-free this type
    // exists to prevent. `<impl Drop for RegisteredBuffers>::drop -> ()`
    // survived M18.3, so nothing observed the guard firing.
    let path = std::env::temp_dir().join(format!("ioring-drop-guard-{}.tmp", std::process::id()));
    std::fs::write(&path, vec![2_u8; 4096]).expect("write fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open fixture");
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&file);

    let mut ring = IoRing::new(16, 16).expect("create ring");
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_buffers(vec![vec![0_u8; 512]])
        .expect("queue buffer registration");
    batch.submit_and_wait(1, 5_000).expect("submit");
    let completion = ring
        .try_pop()
        .expect("pop")
        .expect("registration completed");
    let buffers = pending
        .claim_if(&completion)
        .expect("claims its own")
        .expect("registration succeeded");

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives this operation; the token is leaked below so the
    // buffer stays alive for as long as the kernel may write into it.
    let token = unsafe {
        batch.read_registered_raw(
            handle,
            &buffers,
            crate::RegisteredSpan {
                buffer_index: 0,
                offset: 0,
                len: 512,
            },
            0,
            PushOptions::new(),
        )
    }
    .expect("queue registered read");
    batch.submit_and_wait(1, 5_000).expect("submit");
    // Deliberately never claimed, so the buffer stays outstanding.
    std::mem::forget(token);

    // Dropped last and explicitly: a panic raised while some *other* unwind is
    // already in progress would abort instead of failing the test.
    drop(buffers);
}
