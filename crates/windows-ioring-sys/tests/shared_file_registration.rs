// Copyright (c) 2026 Mike Grier
//! Safe file registration: the ring holds the registered files for its whole
//! life (M30.2, D-81).
//!
//! Whether the ring is holding a handle is observed from outside, by asking the
//! file system for an exclusive open: it fails with a sharing violation while
//! any handle to the file is open, and succeeds once the last one closes. That
//! makes both directions visible -- held while the ring lives, after every
//! caller-side clone is gone, and released once the ring is dropped.

#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::time::Duration;

use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoRing, IoRingErrorExt, PendingFileRegistration, PushOptions,
    RegisteredFiles, SharedFile, WriteCaching,
};
use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

mod common;

/// How long a completion this test caused is allowed to take to arrive. The
/// bound is this crate's own contract (`RS-P-5`), generous because it is not
/// what is under test.
const POP_BOUND: Duration = Duration::from_secs(10);

const LEN: usize = 64;

type Ring = IoRing<Vec<u8>>;

/// A temp file holding `LEN` copies of `fill`, and a `SharedFile` opened on it
/// for read and write. The `TempPath` is returned first so it drops last.
fn fixture(tag: &str, fill: u8) -> (common::TempPath, SharedFile) {
    let path = common::TempPath::new("shared-file-registration", tag);
    std::fs::write(&path, [fill; LEN]).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open for read and write");
    (path, SharedFile::new(OwnedHandle::from(file)))
}

/// Whether some handle to `path` is still open: an exclusive open is refused
/// with a sharing violation exactly then.
fn is_held_open(path: &common::TempPath) -> bool {
    match std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
    {
        Ok(_) => false,
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => true,
        Err(error) => panic!("unexpected error probing {path:?}: {error}"),
    }
}

/// Claim a file registration whose batch has been submitted.
fn claim(ring: &mut Ring, pending: PendingFileRegistration) -> RegisteredFiles {
    let (completion, _held) = ring
        .pop_within(POP_BOUND)
        .expect("pop the registration")
        .expect("the registration completes within the bound");
    pending
        .claim_if(&completion)
        .expect("the completion is the registration's")
        .expect("the registration succeeded")
}

/// Read `LEN` bytes at offset 0 through `file`, safely.
fn read_through(ring: &mut Ring, file: &windows_ioring_sys::RegisteredFile) -> Vec<u8> {
    let mut batch = Batch::new(ring);
    batch
        .read_owned(file, vec![0_u8; LEN], (), 0, PushOptions::new())
        .expect("queue a read through the registered file");
    batch.submit_and_wait(1, 5_000).expect("submit the read");
    let (completion, held) = ring
        .pop_within(POP_BOUND)
        .expect("pop the read")
        .expect("the read completes within the bound");
    assert_eq!(completion.result().expect("the read succeeds"), LEN);
    let (buffer, ()) = held.expect("the ring held the read's buffer");
    buffer.expect("a read carries its buffer")
}

#[test]
fn the_ring_holds_registered_files_open_until_it_is_dropped() {
    let (first_path, first) = fixture("held-first", 0x11);
    let (second_path, second) = fixture("held-second", 0x22);
    let mut ring = Ring::with_inventory(16, 16).expect("create ring");

    // No `unsafe` anywhere in this test: that is what M30.2 adds.
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_shared_files(vec![first, second])
        .expect("queue the registration");
    batch
        .submit_and_wait(1, 5_000)
        .expect("submit the registration");
    let registered = claim(&mut ring, pending);

    // The vector above held the caller's only clones, so nothing but the ring
    // keeps these handles open now.
    assert!(
        is_held_open(&first_path) && is_held_open(&second_path),
        "the ring must hold every registered file open after the caller's clones are gone"
    );

    let first_file = registered.get(0).expect("index 0 exists");
    let second_file = registered.get(1).expect("index 1 exists");
    assert_eq!(
        read_through(&mut ring, &first_file),
        vec![0x11_u8; LEN],
        "index 0 names the first file given"
    );
    assert_eq!(
        read_through(&mut ring, &second_file),
        vec![0x22_u8; LEN],
        "index 1 names the second file given"
    );

    // A write through the registered index lands in the file.
    let mut batch = Batch::new(&mut ring);
    batch
        .write_owned(
            &second_file,
            vec![0x33_u8; LEN],
            (),
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
        .expect("queue a write through the registered file");
    batch.submit_and_wait(1, 5_000).expect("submit the write");
    let (completion, _held) = ring
        .pop_within(POP_BOUND)
        .expect("pop the write")
        .expect("the write completes within the bound");
    assert_eq!(completion.result().expect("the write succeeds"), LEN);
    assert_eq!(
        std::fs::read(&second_path).expect("read the file back"),
        vec![0x33_u8; LEN]
    );

    drop(ring);
    assert!(
        !is_held_open(&first_path) && !is_held_open(&second_path),
        "a quiesced ring must release its registered files once it is dropped"
    );
}

#[test]
fn a_second_file_registration_is_refused_whichever_method_made_the_first() {
    // The safe registration first, then the unsafe one.
    let (_first_path, first) = fixture("second-after-safe", 0);
    let mut ring = Ring::with_inventory(16, 16).expect("create ring");
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_shared_files(vec![first.clone()])
        .expect("queue the first registration");
    batch
        .submit_and_wait(1, 5_000)
        .expect("submit the registration");
    let _registered = claim(&mut ring, pending);
    let handle = {
        let owned = std::fs::File::open(std::env::current_exe().expect("test binary path"))
            .expect("open the test binary");
        OwnedHandle::from(owned)
    };
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` outlives the ring -- and the call is refused before
    // anything is queued, which is what is under test.
    let error = unsafe { batch.register_files(&[handle.as_raw_handle()]) }
        .expect_err("a second registration must be refused");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    drop(batch);
    drop(ring);

    // The unsafe registration first, then the safe one.
    let (_second_path, second) = fixture("second-after-unsafe", 0);
    let mut ring = Ring::with_inventory(16, 16).expect("create ring");
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open until after the ring is dropped below.
    let pending = unsafe { batch.register_files(&[handle.as_raw_handle()]) }
        .expect("queue the first registration");
    batch
        .submit_and_wait(1, 5_000)
        .expect("submit the registration");
    let _registered = claim(&mut ring, pending);
    let mut batch = Batch::new(&mut ring);
    let refused = batch
        .register_shared_files(vec![second])
        .expect_err("a second registration must be refused");
    assert_eq!(refused.error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        refused.payload.map(|files| files.len()),
        Some(1),
        "a refused registration hands its files back (D-80)"
    );
    drop(batch);
    drop(ring);
    drop(handle);
}

#[test]
fn a_shared_file_registration_refused_by_a_full_queue_hands_its_files_back_for_a_retry() {
    let (path, file) = fixture("full-queue", 0x5A);
    let mut ring = Ring::with_inventory(4, 16).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    let mut queued = 0_u32;
    loop {
        match batch.flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default) {
            Ok(_) => queued += 1,
            Err(refused) => {
                assert!(
                    refused.error.is_submission_queue_full(),
                    "the filler must stop at a full queue, got {refused:?}"
                );
                break;
            }
        }
        assert!(queued <= 1024, "the queue never filled");
    }
    let refused = batch
        .register_shared_files(vec![file])
        .expect_err("a full queue must refuse the registration");
    assert!(refused.error.is_submission_queue_full(), "{refused:?}");
    let files = refused
        .payload
        .expect("a refused registration hands its files back");
    assert_eq!(files.len(), 1);
    batch
        .submit_and_wait(queued, 5_000)
        .expect("submit the filler");
    for _ in 0..queued {
        ring.pop_within(POP_BOUND)
            .expect("pop a filler completion")
            .expect("a filler completion arrives within the bound");
    }
    assert_eq!(
        ring.registered_file_count(),
        0,
        "a refused registration does not spend the ring's one registration"
    );

    // The retry, with the files the refusal handed back.
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_shared_files(files)
        .expect("the retry is accepted once the queue has room");
    batch
        .submit_and_wait(1, 5_000)
        .expect("submit the registration");
    let registered = claim(&mut ring, pending);
    let registered_file = registered.get(0).expect("index 0 exists");
    assert_eq!(
        read_through(&mut ring, &registered_file),
        vec![0x5A_u8; LEN]
    );

    drop(ring);
    assert!(!is_held_open(&path), "the ring released the file it held");
}
