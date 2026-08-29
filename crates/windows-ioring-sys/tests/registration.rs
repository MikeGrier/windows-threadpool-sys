// Copyright (c) 2026 Mike Grier
//! End-to-end test of file/buffer registration (M5.3).

#![cfg(windows)]

use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_ioring_sys::{Batch, IoBuf, IoBufMut, IoRing, PushOptions, RegisteredSpan};

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-registration-{tag}-{}.tmp",
        std::process::id()
    ))
}

#[test]
fn a_read_addressing_a_registered_file_and_a_registered_buffer_round_trips() {
    let path = temp_file("round-trip");
    let content = vec![42_u8; 256];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(16, 16).expect("create ring");

    // Register the file handle.
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open for the whole test.
    let files_pending =
        unsafe { batch.register_files(&[handle]) }.expect("queue file registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_files = files_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("file registration succeeded");
    let registered_file = registered_files.get(0).expect("index 0 exists");

    // Register a buffer.
    let mut batch = Batch::new(&mut ring);
    let buffers_pending = batch
        .register_buffers(vec![vec![0_u8; 256]])
        .expect("queue buffer registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_buffers = buffers_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("buffer registration succeeded");

    // A read addressing both by index.
    let mut batch = Batch::new(&mut ring);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 256,
    };
    let token = unsafe {
        batch.read_registered_raw(
            registered_file,
            &registered_buffers,
            span,
            0,
            PushOptions::new(),
        )
    }
    .expect("queue registered read");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");

    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let transferred = completion.result().expect("registered read succeeded");
    assert_eq!(transferred, 256);
    let _ = token
        .claim_if(&completion)
        .expect("token claims its own completion");

    assert_eq!(
        registered_buffers.get(0).expect("buffer 0 exists"),
        &content
    );

    // A registration outliving many operations stays valid: issue several
    // more reads through the same registration.
    for _ in 0..4 {
        let mut batch = Batch::new(&mut ring);
        let token = unsafe {
            batch.read_registered_raw(
                registered_file,
                &registered_buffers,
                span,
                0,
                PushOptions::new(),
            )
        }
        .expect("queue another registered read");
        batch.submit_and_wait(1, 5_000).expect("submit and wait");
        let completion = ring
            .try_pop()
            .expect("pop completion")
            .expect("a completion is ready");
        completion.result().expect("registered read succeeded");
        let _ = token
            .claim_if(&completion)
            .expect("token claims its own completion");
    }

    drop(registered_buffers);
}

#[test]
fn a_second_file_or_buffer_registration_on_the_same_ring_is_refused() {
    // BuildIoRingRegisterFileHandles/BuildIoRingRegisterBuffers replace the
    // whole table rather than appending to it, so a second registration
    // would silently invalidate every index the first one handed out.
    // `Batch::register_files`/`register_buffers` refuse it outright instead.
    let path = temp_file("second-registration-refused");
    std::fs::write(&path, b"content").expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(16, 16).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open for the whole test.
    let files_pending =
        unsafe { batch.register_files(&[handle]) }.expect("queue first file registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let _registered_files = files_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("file registration succeeded");

    let mut batch = Batch::new(&mut ring);
    // SAFETY: as above.
    let error = unsafe { batch.register_files(&[handle]) }
        .expect_err("a second file registration must be refused");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    drop(batch);

    let buffer = vec![0_u8; 8];
    let mut batch = Batch::new(&mut ring);
    let buffers_pending = batch
        .register_buffers(vec![buffer])
        .expect("queue first buffer registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_buffers = buffers_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("buffer registration succeeded");

    let mut batch = Batch::new(&mut ring);
    let error = batch
        .register_buffers(vec![vec![0_u8; 8]])
        .expect_err("a second buffer registration must be refused");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    drop(batch);

    drop(registered_buffers);
}

#[test]
fn a_buffer_registration_survives_heap_churn_between_the_push_and_the_submit() {
    // The kernel reads the IORING_BUFFER_INFO array when the registration op
    // runs -- during SubmitIoRing -- not when BuildIoRingRegisterBuffers
    // returns (D-32, measured). The crate built that array in a local Vec and
    // dropped it before submitting, so the kernel read freed memory and the
    // registration completed with ERROR_NOACCESS.
    //
    // Written against the stated contract rather than the old symptom: it
    // allocates hard between the push and the submit, so any future
    // regression that lets the array die early gets its freed bytes reused
    // rather than left conveniently intact. A plain round-trip test would
    // pass even while broken on a quieter allocator.
    let path = temp_file("bufreg-survives-churn");
    let content = vec![9_u8; 256];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(16, 16).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_buffers(vec![vec![0_u8; 256]])
        .expect("queue buffer registration");

    // Churn the heap between the push and the submit.
    let mut churn: Vec<Vec<u8>> = Vec::new();
    for _ in 0..256 {
        churn.push(vec![0xAA_u8; 256]);
    }
    drop(churn);

    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_buffers = pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("buffer registration must survive heap churn before the submit");

    // And the registration is genuinely usable, not merely reported OK.
    let mut batch = Batch::new(&mut ring);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 256,
    };
    let shared = windows_ioring_sys::SharedFile::new(file.try_clone().expect("clone file").into());
    let token = batch
        .read_registered(&shared, &registered_buffers, span, 0, PushOptions::new())
        .expect("queue read against the registered buffer");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let bytes = completion.result().expect("read succeeded");
    assert_eq!(bytes, 256);
    let _ = token.claim_if(&completion);

    assert_eq!(
        registered_buffers.get(0).expect("buffer 0 exists")[..8],
        content[..8],
        "the read must have landed in the registered buffer"
    );

    drop(registered_buffers);
    let _ = handle;
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_registered_count_is_reserved_at_queue_time_not_confirmed_at_completion() {
    // `registered_file_count` reports what the ring has reserved, not what
    // the kernel has confirmed: it advances when the Build* call queues
    // (M10.3, D-31). Observed here by reading it while the registration is
    // submitted but its completion has not been popped.
    let path = temp_file("reserved-not-confirmed");
    std::fs::write(&path, b"content").expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(16, 16).expect("create ring");
    assert_eq!(ring.registered_file_count(), 0);

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open for the whole test.
    let _pending = unsafe { batch.register_files(&[handle]) }.expect("queue registration");
    batch.submit().expect("submit");

    assert_eq!(
        ring.registered_file_count(),
        1,
        "the count must already be advanced before any completion is observed"
    );

    ring.run_down().expect("drain");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_zero_length_registration_does_not_spend_the_ring_s_one_registration() {
    // The one-shot guard tests the registered *count*, not a flag, so a
    // registration that assigns no index leaves the shot unspent (M10.1,
    // D-28). Asserted against the stated rule rather than against whichever
    // way the kernel happens to answer a zero-length build: if the build is
    // refused the count never advances, and if it is accepted it advances by
    // zero, so either way the following real registration must be accepted.
    let path = temp_file("zero-length-registration");
    std::fs::write(&path, b"content").expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(16, 16).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    // SAFETY: no handle is read at all -- the slice is empty.
    let empty = unsafe { batch.register_files(&[]) };
    drop(batch);
    if empty.is_ok() {
        ring.run_down().expect("drain the zero-length registration");
    }
    assert_eq!(
        ring.registered_file_count(),
        0,
        "a zero-length registration must not advance the base index"
    );

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open for the whole test.
    let pending = unsafe { batch.register_files(&[handle]) }
        .expect("a real registration must still be accepted after a zero-length one");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered = pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("file registration succeeded");
    assert_eq!(registered.len(), 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn dropping_a_registration_with_an_operation_in_flight_leaks_rather_than_frees() {
    /// A buffer that records whether its destructor ran, so the test can
    /// distinguish "leaked (forgotten)" from "dropped (freed)" -- the exact
    /// distinction M5.3 exists to get right, mirroring `Token`'s own test.
    struct DropTracking {
        data: Vec<u8>,
        dropped: Arc<AtomicBool>,
    }

    impl Drop for DropTracking {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    // SAFETY: the bytes live in `data`'s heap allocation, independent of
    // where this wrapper struct sits; the length is fixed once constructed.
    unsafe impl IoBuf for DropTracking {
        fn stable_ptr(&self) -> *const u8 {
            self.data.as_ptr()
        }

        fn bytes_len(&self) -> usize {
            self.data.len()
        }
    }

    // SAFETY: `&mut self` proves exclusive access; same allocation as
    // `stable_ptr`.
    unsafe impl IoBufMut for DropTracking {
        fn stable_mut_ptr(&mut self) -> *mut u8 {
            self.data.as_mut_ptr()
        }
    }

    let path = temp_file("refused-drop");
    let content = vec![7_u8; 64];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(8, 8).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    let dropped = Arc::new(AtomicBool::new(false));
    let buffer = DropTracking {
        data: vec![0_u8; 64],
        dropped: Arc::clone(&dropped),
    };
    let buffers_pending = batch
        .register_buffers(vec![buffer])
        .expect("queue buffer registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_buffers = buffers_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("buffer registration succeeded");

    let mut batch = Batch::new(&mut ring);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 64,
    };
    let token = unsafe {
        batch.read_registered_raw(handle, &registered_buffers, span, 0, PushOptions::new())
    }
    .expect("queue registered read");
    batch.submit_and_wait(0, 0).expect("submit without waiting");
    // Deliberately do not observe this read's completion, so
    // `registered_buffers`'s outstanding count is still 1 when dropped below.

    // In a debug build this deliberately triggers `RegisteredBuffers`'s own
    // `debug_assert!` (a caller bug, on purpose) -- `catch_unwind` lets the
    // test keep running rather than aborting on it; a release build hits
    // neither the panic nor this branch, since `debug_assert!` is a no-op
    // there.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(registered_buffers);
    }));
    assert!(
        !dropped.load(Ordering::SeqCst),
        "RegisteredBuffers dropped with an operation in flight must leak its buffers, not free them"
    );

    // Let the real, still-outstanding read finish before the ring itself
    // tears down, so `IoRing::drop`'s own rundown does not have to.
    ring.run_down()
        .expect("run down the outstanding registered read");
    drop(token);
}

#[test]
fn a_registered_file_from_a_different_ring_is_rejected() {
    let path = temp_file("cross-ring-file");
    std::fs::write(&path, b"content").expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring_a = IoRing::new(8, 8).expect("create ring a");
    let mut ring_b = IoRing::new(8, 8).expect("create ring b");

    let mut batch = Batch::new(&mut ring_a);
    // SAFETY: `handle` stays open for the whole test.
    let files_pending =
        unsafe { batch.register_files(&[handle]) }.expect("queue file registration on ring a");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring_a
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_files = files_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("file registration succeeded");
    let registered_file = registered_files.get(0).expect("index 0 exists");

    // `registered_file`'s index is only meaningful against ring a's own
    // file table; pushing it through ring b must be refused rather than
    // silently addressing whatever (or nothing) sits at that index there.
    let mut batch = Batch::new(&mut ring_b);
    let buffer = vec![0_u8; 8];
    // SAFETY: never actually queued -- the ring-identity check rejects this
    // before any `Build*` call runs.
    let error = unsafe { batch.read_raw(registered_file, buffer, 0, PushOptions::new()) }
        .expect_err("a RegisteredFile from a different ring must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn a_registered_file_is_readable_through_the_safe_api_without_unsafe() {
    // M10.4 / D-29: a RegisteredFile carries no lifetime obligation -- the
    // index names a table the ring owns and was checked against the minting
    // ring -- so reaching it must not require an `unsafe` call whose contract
    // is vacuous for exactly this input. Note the absence of `unsafe` below:
    // that absence is the whole point of the test.
    let path = temp_file("safe-registered-file");
    let content = vec![13_u8; 128];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(16, 16).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open for the whole test. (Registering is still
    // unsafe -- the handles must outlive the ring -- it is only *using* the
    // resulting index that M10.4 made safe.)
    let files_pending =
        unsafe { batch.register_files(&[handle]) }.expect("queue file registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_file = files_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("file registration succeeded")
        .get(0)
        .expect("index 0 exists");

    let mut batch = Batch::new(&mut ring);
    let token = batch
        .read(&registered_file, vec![0_u8; 128], 0, PushOptions::new())
        .expect("queue a safe read against the registered file");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    assert_eq!(completion.result().expect("read succeeded"), 128);
    let (buffer, returned_file) = token
        .claim_if(&completion)
        .expect("token claims completion");
    assert_eq!(buffer, content);
    assert_eq!(
        returned_file, registered_file,
        "the guard hands the registered file back unchanged"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_registered_file_and_a_registered_buffer_compose_through_the_safe_api() {
    // The fully-registered form -- registered file *and* registered buffer --
    // which before M10.4 the safe API could not express at all (D-29).
    let path = temp_file("safe-fully-registered");
    let content = vec![21_u8; 64];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(16, 16).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open for the whole test.
    let files_pending =
        unsafe { batch.register_files(&[handle]) }.expect("queue file registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_file = files_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("file registration succeeded")
        .get(0)
        .expect("index 0 exists");

    let mut batch = Batch::new(&mut ring);
    let buffers_pending = batch
        .register_buffers(vec![vec![0_u8; 64]])
        .expect("queue buffer registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_buffers = buffers_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("buffer registration succeeded");

    let mut batch = Batch::new(&mut ring);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 64,
    };
    let token = batch
        .read_registered(
            &registered_file,
            &registered_buffers,
            span,
            0,
            PushOptions::new(),
        )
        .expect("queue a safe fully-registered read");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    assert_eq!(completion.result().expect("read succeeded"), 64);
    let (registered_use, returned_file) = token
        .claim_if(&completion)
        .expect("token claims completion");
    assert_eq!(returned_file, registered_file);
    drop(registered_use);

    assert_eq!(
        registered_buffers.get(0).expect("buffer 0 exists"),
        &content,
        "the read must have landed in the registered buffer"
    );

    drop(registered_buffers);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_safe_api_rejects_a_registered_file_from_a_different_ring() {
    // The cross-ring check (D-17) still applies on the safe path: making
    // RegisteredFile reachable without `unsafe` must not weaken it.
    let path = temp_file("safe-cross-ring-file");
    std::fs::write(&path, b"content").expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring_a = IoRing::new(8, 8).expect("create ring a");
    let mut ring_b = IoRing::new(8, 8).expect("create ring b");

    let mut batch = Batch::new(&mut ring_a);
    // SAFETY: `handle` stays open for the whole test.
    let files_pending =
        unsafe { batch.register_files(&[handle]) }.expect("queue file registration on ring a");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring_a
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_file = files_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("file registration succeeded")
        .get(0)
        .expect("index 0 exists");

    let mut batch = Batch::new(&mut ring_b);
    let error = batch
        .read(&registered_file, vec![0_u8; 8], 0, PushOptions::new())
        .expect_err("a RegisteredFile from a different ring must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    drop(batch);

    // And the rejection cost ring b nothing: no identity was reserved, so
    // nothing counts against its rundown (M10.1).
    assert_eq!(ring_b.outstanding(), 0);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_registered_buffers_from_a_different_ring_is_rejected() {
    let path = temp_file("cross-ring-buffers");
    let content = vec![9_u8; 32];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring_a = IoRing::new(8, 8).expect("create ring a");
    let mut ring_b = IoRing::new(8, 8).expect("create ring b");

    let mut batch = Batch::new(&mut ring_a);
    let buffers_pending = batch
        .register_buffers(vec![vec![0_u8; 32]])
        .expect("queue buffer registration on ring a");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring_a
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    let registered_buffers = buffers_pending
        .claim_if(&completion)
        .expect("id matches")
        .expect("buffer registration succeeded");

    // `registered_buffers`'s index space is only meaningful against ring
    // a's own buffer table; addressing it through ring b must be refused.
    let mut batch = Batch::new(&mut ring_b);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 32,
    };
    // SAFETY: never actually queued -- the ring-identity check rejects this
    // before any `Build*` call runs.
    let error = unsafe {
        batch.read_registered_raw(handle, &registered_buffers, span, 0, PushOptions::new())
    }
    .expect_err("a RegisteredBuffers from a different ring must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

    drop(registered_buffers);
}

#[test]
fn dropping_an_unclaimed_pending_buffer_registration_leaks_rather_than_frees() {
    /// As `dropping_a_registration_with_an_operation_in_flight_...`'s own
    /// helper: distinguishes "leaked (forgotten)" from "dropped (freed)".
    struct DropTracking {
        data: Vec<u8>,
        dropped: Arc<AtomicBool>,
    }

    impl Drop for DropTracking {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    // SAFETY: the bytes live in `data`'s heap allocation, independent of
    // where this wrapper struct sits; the length is fixed once constructed.
    unsafe impl IoBuf for DropTracking {
        fn stable_ptr(&self) -> *const u8 {
            self.data.as_ptr()
        }

        fn bytes_len(&self) -> usize {
            self.data.len()
        }
    }

    // SAFETY: `&mut self` proves exclusive access; same allocation as
    // `stable_ptr`.
    unsafe impl IoBufMut for DropTracking {
        fn stable_mut_ptr(&mut self) -> *mut u8 {
            self.data.as_mut_ptr()
        }
    }

    let mut ring = IoRing::new(8, 8).expect("create ring");
    let dropped = Arc::new(AtomicBool::new(false));
    let buffer = DropTracking {
        data: vec![0_u8; 64],
        dropped: Arc::clone(&dropped),
    };

    let mut batch = Batch::new(&mut ring);
    let buffers_pending = batch
        .register_buffers(vec![buffer])
        .expect("queue buffer registration");
    batch.submit_and_wait(0, 0).expect("submit without waiting");
    // Deliberately dropped without ever calling `claim_if`: the registration
    // is already queued via `BuildIoRingRegisterBuffers`, so nothing here
    // proves the kernel is done deciding whether to retain these addresses.
    drop(buffers_pending);
    assert!(
        !dropped.load(Ordering::SeqCst),
        "an unclaimed PendingBufferRegistration must leak its buffers, not free them"
    );

    // Let the real, still-outstanding registration finish before the ring
    // itself tears down, so `IoRing::drop`'s own rundown does not have to.
    ring.run_down()
        .expect("run down the outstanding registration");
    // The leak is permanent: nothing later runs the destructor either,
    // including the ring's own teardown.
    assert!(!dropped.load(Ordering::SeqCst));
}
