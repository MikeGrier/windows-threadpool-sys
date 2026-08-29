// Copyright (c) 2026 Mike Grier
//! End-to-end submission-lifecycle tests against real files (M3.6).

#![cfg(windows)]

use std::collections::HashMap;
use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::PathBuf;

use windows_ioring_sys::{
    Batch, IoRing, IoRingErrorExt, PushOptions, RingCondition, SharedFile, Token,
};
use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;

const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 512;

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-submission-{tag}-{}.tmp",
        std::process::id()
    ))
}

fn filled_content() -> Vec<u8> {
    let mut content = vec![0_u8; CHUNKS * CHUNK_LEN];
    for (chunk_index, chunk) in content.chunks_mut(CHUNK_LEN).enumerate() {
        chunk.fill(chunk_index as u8);
    }
    content
}

fn error_code(error: &io::Error) -> windows_sys::core::HRESULT {
    error
        .as_ioring_error()
        .expect("error is an IoRingError")
        .code()
}

#[test]
fn many_reads_round_trip_every_user_data_and_buffer() {
    let path = temp_file("round-trip");
    let content = filled_content();
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(64, 64).expect("create ring");
    let mut pending: HashMap<usize, (usize, Token<Vec<u8>>)> = HashMap::new();
    {
        let mut batch = Batch::new(&mut ring);
        for chunk_index in 0..CHUNKS {
            let buffer = vec![0_u8; CHUNK_LEN];
            let offset = (chunk_index * CHUNK_LEN) as u64;
            let token = unsafe { batch.read_raw(handle, buffer, offset, PushOptions::new()) }
                .expect("queue read");
            pending.insert(token.id(), (chunk_index, token));
        }
        batch
            .submit_and_wait(CHUNKS as u32, 5_000)
            .expect("submit and wait");
    }

    let mut attempts = 0;
    while !pending.is_empty() {
        attempts += 1;
        assert!(
            attempts <= CHUNKS * 4,
            "expected all completions ready after submit_and_wait"
        );
        let Some(completion) = ring.try_pop().expect("pop completion") else {
            continue;
        };
        let user_data = completion.user_data();
        let transferred = completion.result().expect("read succeeded");
        let (chunk_index, token) = pending
            .remove(&user_data)
            .expect("completion matches a held token");
        let buffer = token
            .claim_if(&completion)
            .expect("a token claims its own completion");
        assert_eq!(transferred, CHUNK_LEN);
        assert_eq!(
            buffer,
            content[chunk_index * CHUNK_LEN..(chunk_index + 1) * CHUNK_LEN]
        );
    }
}

#[test]
fn pushing_past_submission_queue_capacity_reports_backpressure_and_the_ring_stays_usable() {
    let mut ring = IoRing::new(4, 64).expect("create ring");
    let capacity = ring.info().expect("info").submission_queue_size;
    let path = temp_file("backpressure");
    std::fs::write(&path, b"").expect("create fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open");
    let handle = file.as_raw_handle();

    let mut queued = 0_u32;
    let overflow_error = {
        let mut batch = Batch::new(&mut ring);
        loop {
            // SAFETY: `handle` stays open for the whole test.
            match unsafe { batch.flush_raw(handle, PushOptions::new()) } {
                Ok(_user_data) => {
                    queued += 1;
                    assert!(
                        queued <= capacity + 1,
                        "expected backpressure at or before capacity + 1 pushes"
                    );
                }
                Err(error) => break error,
            }
        }
        // `batch` drops here, submitting the `queued` flushes that did
        // succeed (D-5) -- the point under test is that the ring stays
        // usable afterward, not that the overfilled batch is discarded.
    };

    assert_eq!(
        queued, capacity,
        "backpressure must trip exactly at the negotiated queue capacity"
    );
    // Asked through the named predicate rather than a hand-rolled downcast
    // plus HRESULT comparison (M10.5, D-30). This binds the predicate to a
    // *real* kernel-reported queue-full rather than a synthetic
    // `IoRingError`, which is what the unit tests in `error/tests.rs` cover.
    assert!(
        overflow_error.is_submission_queue_full(),
        "the backpressure signal every push's docs name must be what the \
         predicate reports: got {:?}",
        overflow_error.ring_condition()
    );
    assert_eq!(
        overflow_error.ring_condition(),
        Some(RingCondition::SubmissionQueueFull)
    );
    // And `kind()` still cannot answer -- the asymmetry D-30 records.
    assert_eq!(overflow_error.kind(), io::ErrorKind::Other);

    let mut remaining = queued;
    while remaining > 0 {
        if ring.try_pop().expect("pop completion").is_some() {
            remaining -= 1;
        }
    }

    // The ring stays usable: push and submit once more, cleanly.
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `handle` stays open for the whole test.
    let user_data = unsafe { batch.flush_raw(handle, PushOptions::new()) }
        .expect("ring still accepts pushes after backpressure");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    assert_eq!(completion.user_data(), user_data);
    completion.result().expect("flush succeeded");
}

#[test]
fn a_dropped_batch_still_submits_its_queued_operations() {
    let path = temp_file("dropped-batch");
    let content = vec![7_u8; 64];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(8, 8).expect("create ring");
    let buffer = vec![0_u8; content.len()];
    let token = {
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `handle` stays open for the whole test.
        unsafe { batch.read_raw(handle, buffer, 0, PushOptions::new()) }.expect("queue read")
        // `batch` drops here without an explicit `submit()` call (D-5).
    };
    let user_data = token.id();

    // A fresh batch that queues nothing still waits on the whole ring's
    // completion queue: if the dropped batch above had not actually
    // submitted its read, this would time out.
    Batch::new(&mut ring)
        .submit_and_wait(1, 5_000)
        .expect("submit and wait");

    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    assert_eq!(completion.user_data(), user_data);
    assert_eq!(completion.result().expect("read succeeded"), content.len());
    let buffer = token
        .claim_if(&completion)
        .expect("a token claims its own completion");
    assert_eq!(buffer, content);
}

#[test]
fn cancelling_a_target_that_is_not_outstanding_reports_error_not_found_through_completion() {
    let path = temp_file("cancel-not-found");
    std::fs::write(&path, b"").expect("create fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open");
    let handle = file.as_raw_handle();

    let mut ring = IoRing::new(8, 8).expect("create ring");
    let cancel_user_data = {
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `handle` stays open for the whole test.
        let user_data = unsafe { batch.cancel_raw(handle, 999_999) }.expect("queue cancel");
        batch.submit_and_wait(1, 5_000).expect("submit and wait");
        user_data
    };

    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    assert_eq!(completion.user_data(), cancel_user_data);
    let error = completion
        .result()
        .expect_err("a target that was never outstanding must fail");
    assert_eq!(
        (error_code(&error) as u32) & 0xFFFF,
        ERROR_NOT_FOUND,
        "expected ERROR_NOT_FOUND in the HRESULT"
    );
}

#[test]
fn dropping_the_callers_own_sharedfile_clone_does_not_close_a_still_outstanding_handle() {
    let path = temp_file("shared-file");
    let content = vec![9_u8; CHUNK_LEN];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");

    let mut ring = IoRing::new(8, 8).expect("create ring");
    let shared = SharedFile::new(OwnedHandle::from(file));
    let buffer = vec![0_u8; CHUNK_LEN];
    let token = {
        let mut batch = Batch::new(&mut ring);
        let token = batch
            .read(&shared, buffer, 0, PushOptions::new())
            .expect("queue shared read");
        batch.submit_and_wait(0, 0).expect("submit without waiting");
        token
    };

    // Drop the caller's own SharedFile clone -- its only external
    // reference. If the token's own clone were not keeping the underlying
    // handle open, the read below would fail against a closed handle.
    drop(shared);

    Batch::new(&mut ring)
        .submit_and_wait(1, 5_000)
        .expect("submit and wait");
    let completion = ring
        .try_pop()
        .expect("pop completion")
        .expect("a completion is ready");
    assert_eq!(
        completion
            .result()
            .expect("read succeeded against a still-open handle"),
        CHUNK_LEN
    );
    let (buffer, _file) = token
        .claim_if(&completion)
        .expect("token claims its own completion");
    assert_eq!(buffer, content);
}
