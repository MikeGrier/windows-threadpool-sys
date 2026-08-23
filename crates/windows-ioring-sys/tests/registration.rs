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
        batch.read_registered(
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
            batch.read_registered(
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
    let token =
        unsafe { batch.read_registered(handle, &registered_buffers, span, 0, PushOptions::new()) }
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
