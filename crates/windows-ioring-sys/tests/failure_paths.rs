// Copyright (c) 2026 Mike Grier
//! The failure paths, which nothing had ever executed (M16.4).
//!
//! Every other test in this crate exercises operations that succeed. That is
//! not an oversight anyone chose -- a healthy machine simply does not fail a
//! read of a temporary file on demand -- but the consequence is that error
//! handling is written once and then never run again, which is exactly where
//! this repository's defects have lived. `Appender::claim` returned early on a
//! failed write and leaked its arena slot permanently, on a branch no test had
//! ever taken. The epoch log's checkpoint path recorded a failed write and then
//! authorised a reclaim on it anyway.
//!
//! # Assert the documented degradation, not merely the absence of a panic
//!
//! Both defects above would have passed a test that only checked "nothing
//! blew up". They were failures that were *noticed* and then not acted on. So
//! each test here names the specific promise the rustdoc makes about failure
//! and holds the code to it.
//!
//! # Real failures where they are reachable
//!
//! Cancelling something that is not outstanding is a genuine, reliably
//! reproducible kernel error, so the `EventDelivery` test uses one rather than
//! an injected one -- it exercises the crate's own error translation as well as
//! the delivery path. Injection is for the failures a healthy machine will not
//! produce.

#![cfg(all(windows, feature = "fault-injection"))]

use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use windows_ioring_sys::contract::{RingContract, Violation};
use windows_ioring_sys::{
    Batch, Completion, FlushCoverage, FlushMode, InjectedFailure, IoBuf, IoBufMut, IoRing,
    IoRingErrorExt, PushOptions, RingCondition, SharedFile, WriteCaching,
};

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-failure-paths-{tag}-{}.tmp",
        std::process::id()
    ))
}

/// Drain until a completion arrives.
fn await_one(ring: &mut IoRing) -> Completion {
    loop {
        if let Some(completion) = ring.try_pop().expect("pop a completion") {
            return completion;
        }
        Batch::new(ring)
            .submit_and_wait(1, 30_000)
            .expect("wait for a completion");
    }
}

#[test]
fn a_failed_read_still_hands_its_buffer_back_when_claimed() {
    // The documented promise: `claim_if` matches on identity, not on outcome.
    // A caller that only claims successful completions leaks the buffer of
    // every failed one -- `Token`'s drop deliberately forgets rather than
    // frees, which is what keeps the kernel's pointer valid and what makes the
    // leak permanent.
    let path = temp_file("read");
    std::fs::write(&path, b"hello").expect("create the fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open the fixture");

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let mut contract = RingContract::new();
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives the operation; the token is claimed below.
    let token =
        unsafe { batch.read_raw(file.as_raw_handle(), vec![0_u8; 5], 0, PushOptions::new()) }
            .expect("queue a read");
    contract.observe_push(token.id());
    batch.submit_and_wait(1, 30_000).expect("submit and wait");

    let completion = await_one(&mut ring);
    contract.observe_completion(completion.user_data());
    let failed = completion.with_injected_failure(InjectedFailure::Ring(RingCondition::Corrupt));

    assert!(failed.result().is_err(), "the failure applies");
    let buffer = token
        .claim_if(&failed)
        .expect("a failed completion names its own token exactly as a successful one does");
    contract.observe_claim(failed.user_data());

    assert_eq!(
        buffer, b"hello",
        "the buffer comes back on the failure path too -- claiming is what returns it"
    );
    contract.assert_quiescent();

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_failed_write_still_hands_its_buffer_back_when_claimed() {
    // The same promise on the write side. This is the exact shape of the
    // `Appender::claim` defect: that code checked the write's result *before*
    // claiming, and returned early on failure, so the token dropped unclaimed
    // and its registered-buffer slot was gone for the process's life.
    let path = temp_file("write");
    std::fs::write(&path, vec![0_u8; 16]).expect("create the fixture");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open the fixture");

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let mut contract = RingContract::new();
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives the operation; the token is claimed below.
    let token = unsafe {
        batch.write_raw(
            file.as_raw_handle(),
            vec![7_u8; 4],
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
    }
    .expect("queue a write");
    contract.observe_push(token.id());
    batch.submit_and_wait(1, 30_000).expect("submit and wait");

    let completion = await_one(&mut ring);
    contract.observe_completion(completion.user_data());
    let failed = completion.with_injected_failure(InjectedFailure::Win32(
        windows_sys::Win32::Foundation::ERROR_DISK_FULL,
    ));

    assert!(failed.result().is_err(), "the failure applies");
    let buffer = token.claim_if(&failed).expect("claims its own completion");
    contract.observe_claim(failed.user_data());

    assert_eq!(buffer, vec![7_u8; 4], "the source buffer comes back");
    contract.assert_quiescent();

    let _ = std::fs::remove_file(&path);
}

#[test]
fn claiming_before_checking_the_result_is_what_stops_a_failure_from_leaking() {
    // The M16.2 finding, now reachable. Reverting `Appender::claim`'s *fix*
    // does not reproduce its defect, because the early return only fires on a
    // failed write -- a path nothing could reach until this seam existed.
    //
    // Both orderings are run here against the same conservation oracle, so the
    // difference between them is a reported violation rather than an argument.
    let path = temp_file("ordering");
    std::fs::write(&path, b"hello").expect("create the fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open the fixture");

    // Ordering A -- check the result first, return early on failure. The token
    // is dropped unclaimed, which `Token` treats as "still outstanding" and
    // forgets.
    let leaked = {
        let mut ring = IoRing::new(16, 16).expect("create a ring");
        let mut contract = RingContract::new();
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `file` outlives the operation.
        let token =
            unsafe { batch.read_raw(file.as_raw_handle(), vec![0_u8; 5], 0, PushOptions::new()) }
                .expect("queue a read");
        contract.observe_push(token.id());
        batch.submit_and_wait(1, 30_000).expect("submit and wait");

        let completion = await_one(&mut ring);
        contract.observe_completion(completion.user_data());
        let failed =
            completion.with_injected_failure(InjectedFailure::Ring(RingCondition::Corrupt));

        let id = token.id();
        if failed.result().is_err() {
            // The bug: bail out before claiming.
            drop(token);
        }
        (id, contract.check_quiescent())
    };
    assert_eq!(
        leaked.1,
        vec![Violation::LeakedToken {
            user_data: leaked.0
        }],
        "checking the result before claiming must leak the token on failure"
    );

    // Ordering B -- claim first, then check. The completion has already been
    // observed, so claiming is sound either way, and the buffer comes back
    // before any early return can skip it.
    let clean = {
        let mut ring = IoRing::new(16, 16).expect("create a ring");
        let mut contract = RingContract::new();
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `file` outlives the operation.
        let token =
            unsafe { batch.read_raw(file.as_raw_handle(), vec![0_u8; 5], 0, PushOptions::new()) }
                .expect("queue a read");
        contract.observe_push(token.id());
        batch.submit_and_wait(1, 30_000).expect("submit and wait");

        let completion = await_one(&mut ring);
        contract.observe_completion(completion.user_data());
        let failed =
            completion.with_injected_failure(InjectedFailure::Ring(RingCondition::Corrupt));

        let _buffer = token.claim_if(&failed).expect("claims its own completion");
        contract.observe_claim(failed.user_data());
        let _ = failed.result();
        contract.check_quiescent()
    };
    assert_eq!(
        clean,
        Vec::new(),
        "claiming before checking must conserve the token on the failure path"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_failed_flush_reports_its_error_and_leaves_the_ring_usable() {
    // A flush carries no token, so there is nothing to leak -- the promise
    // here is narrower and still worth binding: a failed operation is one
    // operation, not a poisoned ring.
    let path = temp_file("flush");
    std::fs::write(&path, b"x").expect("create the fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open the fixture");

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let mut contract = RingContract::new();

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives both operations.
    let first = unsafe {
        batch.flush_raw(
            file.as_raw_handle(),
            FlushCoverage::Unordered,
            FlushMode::Default,
        )
    }
    .expect("queue a flush");
    contract.observe_tokenless_push(first);
    batch.submit_and_wait(1, 30_000).expect("submit and wait");

    let completion = await_one(&mut ring);
    contract.observe_completion(completion.user_data());
    let failed = completion.with_injected_failure(InjectedFailure::Win32(
        windows_sys::Win32::Foundation::ERROR_WRITE_FAULT,
    ));
    assert_eq!(
        (failed
            .result()
            .expect_err("the failure applies")
            .as_ioring_error()
            .expect("an IoRingError")
            .code() as u32)
            & 0xFFFF,
        windows_sys::Win32::Foundation::ERROR_WRITE_FAULT
    );

    // The ring still works: push, submit and complete another operation.
    let mut batch = Batch::new(&mut ring);
    // SAFETY: as above.
    let second = unsafe {
        batch.flush_raw(
            file.as_raw_handle(),
            FlushCoverage::Unordered,
            FlushMode::Default,
        )
    }
    .expect("a failed operation must not stop the ring accepting pushes");
    contract.observe_tokenless_push(second);
    batch.submit_and_wait(1, 30_000).expect("submit and wait");
    let completion = await_one(&mut ring);
    contract.observe_completion(completion.user_data());
    completion
        .result()
        .expect("the second flush succeeds on its own merits");

    contract.assert_quiescent();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_failed_registration_drops_its_buffers_rather_than_leaking_them() {
    // The documented degradation, from `PendingBufferRegistration::claim_if`:
    // "the buffers are dropped normally in that case, exactly as if they had
    // never been registered -- a matched completion, success or failure, is
    // exactly the proof this type's `Drop` is waiting for."
    //
    // That is the *opposite* of the unclaimed-drop case, which leaks on
    // purpose, and it had never been executed because a registration failure
    // is not reachable on a healthy machine.
    //
    // PRECAUTION, and it is load-bearing: injecting a failure here frees
    // buffers the kernel genuinely does hold registered, because the
    // registration really did succeed. That is inert only while nothing uses
    // it, so this ring takes no registered-buffer operation afterwards and is
    // dropped immediately. See `Completion::with_injected_failure`.
    struct DropTracking {
        data: Vec<u8>,
        dropped: Arc<AtomicBool>,
    }

    impl Drop for DropTracking {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    // SAFETY: the bytes live in `data`'s own heap allocation, independent of
    // where this wrapper sits; the length is fixed once constructed.
    unsafe impl IoBuf for DropTracking {
        fn stable_ptr(&self) -> *const u8 {
            self.data.as_ptr()
        }
        fn bytes_len(&self) -> usize {
            self.data.len()
        }
    }
    // SAFETY: as above, and the allocation is uniquely owned.
    unsafe impl IoBufMut for DropTracking {
        fn stable_mut_ptr(&mut self) -> *mut u8 {
            self.data.as_mut_ptr()
        }
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_buffers(vec![DropTracking {
            data: vec![0_u8; 128],
            dropped: Arc::clone(&dropped),
        }])
        .expect("queue the registration");
    batch.submit_and_wait(1, 30_000).expect("submit and wait");

    let completion = await_one(&mut ring);
    assert!(
        !dropped.load(Ordering::SeqCst),
        "the buffer is still held while the registration is unclaimed"
    );

    let failed = completion.with_injected_failure(InjectedFailure::Ring(RingCondition::Corrupt));
    let outcome = pending
        .claim_if(&failed)
        .expect("the registration claims its own completion");
    assert!(
        outcome.is_err(),
        "a failed registration must report the failure rather than hand back buffers"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "a claimed-but-failed registration must DROP its buffers, not leak them -- \
         the matched completion is the proof its Drop was waiting for"
    );

    // Precaution: nothing registered runs on this ring, and it dies here.
    drop(ring);
}

#[test]
fn event_delivery_hands_a_failed_completion_to_the_callback() {
    // A genuine failure, not an injected one: cancelling a target that is not
    // outstanding reports ERROR_NOT_FOUND reliably, so this exercises the
    // crate's own error translation as well as the delivery path.
    //
    // The promise: `EventDelivery` delivers every completion. It does not
    // inspect results, and must not quietly swallow a failed one -- a consumer
    // that never sees the failure cannot handle it.
    let path = temp_file("delivery");
    std::fs::write(&path, b"x").expect("create the fixture");
    let file = SharedFile::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open the fixture")
            .into(),
    );

    let ring = IoRing::new(16, 16).expect("create a ring");
    let delivered = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(AtomicUsize::new(0));
    let not_found = Arc::new(std::sync::atomic::AtomicIsize::new(0));

    let seen = Arc::clone(&delivered);
    let failed = Arc::clone(&failures);
    let saw_code = Arc::clone(&not_found);
    let delivery = windows_ioring_sys::EventDelivery::new(
        ring,
        move |completion| {
            seen.fetch_add(1, Ordering::SeqCst);
            if let Err(error) = completion.result() {
                // The code is published *before* the counter the waiting
                // thread spins on, not after. The counter is what says
                // "everything about this failure is now visible", so storing
                // the code afterwards would let the waiter read a stale zero
                // -- which is exactly what the first version of this test did.
                if let Some(ring_error) = error.as_ioring_error() {
                    saw_code.store(ring_error.code() as isize, Ordering::SeqCst);
                }
                failed.fetch_add(1, Ordering::SeqCst);
            }
        },
        None,
    )
    .expect("the completion event is available");

    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        // A cancel naming a `UserData` that is not outstanding: a real error,
        // reported through the completion rather than at push time.
        let token = batch.cancel(&file, 0xDEAD_BEEF).expect("queue the cancel");
        batch.submit().expect("submit the cancel");

        // Deliberately abandoned. The completion is delivered to a pool
        // thread, so this thread never gets one to claim against -- and
        // `Token`'s drop forgets rather than frees, which is what keeps the
        // file guard alive for as long as the kernel might need it. The ring's
        // own `outstanding` count is decremented at *pop*, not at claim, so
        // nothing here blocks rundown.
        drop(token);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while failures.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }

    assert!(
        delivered.load(Ordering::SeqCst) > 0,
        "the callback must receive the completion at all"
    );
    assert_eq!(
        failures.load(Ordering::SeqCst),
        1,
        "a failed completion must be delivered, not swallowed"
    );
    assert_eq!(
        (not_found.load(Ordering::SeqCst) as u32) & 0xFFFF,
        windows_sys::Win32::Foundation::ERROR_NOT_FOUND,
        "the error must survive delivery intact, as ERROR_NOT_FOUND"
    );

    drop(delivery);
    let _ = std::fs::remove_file(&path);
}
