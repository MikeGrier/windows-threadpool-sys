// Copyright (c) 2026 Mike Grier
//! A refused owned push hands back what it was given (M30.1, D-80).
//!
//! Every case here is refused by a **full submission queue**, because that is
//! the refusal a caller cannot rule out beforehand and the one where getting
//! the buffer back matters: the documented answer is to submit and retry, and
//! the retry wants the same buffer. The refusals made before the build -- an
//! oversized buffer, a foreign registration -- are covered beside the tests
//! that already exercised them, in `submission_lifecycle.rs`,
//! `registration.rs` and `ring_owned_inventory.rs`.

#![cfg(windows)]

use std::os::windows::io::OwnedHandle;
use std::time::Duration;

use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoRing, IoRingErrorExt, PushOptions, RegisteredSpan,
    SharedFile, WriteCaching,
};

mod common;

/// How long a completion this test caused is allowed to take to arrive. The
/// bound is this crate's own contract (`RS-P-5`), generous because it is not
/// what is under test.
const POP_BOUND: Duration = Duration::from_secs(10);

/// The sidecar every filler flush carries, distinct from the one under test.
const FILLER: u32 = 0;

/// The sidecar the refused push carries.
const REFUSED: u32 = 7;

const LEN: usize = 64;

type Ring = IoRing<Vec<u8>, u32>;

/// A temp file holding `LEN` zero bytes, opened for read and write.
fn fixture(tag: &str) -> (common::TempPath, SharedFile) {
    let path = common::TempPath::new("push-refusal", tag);
    std::fs::write(&path, [0_u8; LEN]).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open for read and write");
    (path, SharedFile::new(OwnedHandle::from(file)))
}

/// Queue unordered flushes until the submission queue refuses one, and return
/// how many were queued.
fn fill(batch: &mut Batch<'_, Vec<u8>, u32>, file: &SharedFile) -> u32 {
    let mut queued = 0_u32;
    loop {
        match batch.flush_owned(file, FILLER, FlushCoverage::Unordered, FlushMode::Default) {
            Ok(_) => queued += 1,
            Err(refused) => {
                assert!(
                    refused.error.is_submission_queue_full(),
                    "the filler must stop at a full queue, got {refused:?}"
                );
                return queued;
            }
        }
        assert!(queued <= 1024, "the queue never filled");
    }
}

/// Pop the `count` filler completions.
fn drain(ring: &mut Ring, count: u32) {
    for _ in 0..count {
        let (_completion, held) = ring
            .pop_within(POP_BOUND)
            .expect("pop a filler completion")
            .expect("a filler completion arrives within the bound");
        let (payload, extra) = held.expect("the ring held the filler flush");
        assert!(payload.is_none(), "a flush carries no payload");
        assert_eq!(extra, FILLER);
    }
}

#[test]
fn a_write_refused_by_a_full_queue_hands_back_the_same_buffer_and_sidecar_for_a_retry() {
    let (path, file) = fixture("write");
    let mut ring = Ring::with_inventory(4, 64).expect("create ring");

    let buffer = vec![0xA5_u8; LEN];
    let address = buffer.as_ptr();
    let mut batch = Batch::new(&mut ring);
    let queued = fill(&mut batch, &file);
    let refused = batch
        .write_owned(
            &file,
            buffer,
            REFUSED,
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
        .expect_err("a full queue must refuse the write");
    assert!(refused.error.is_submission_queue_full(), "{refused:?}");
    assert_eq!(refused.extra, REFUSED, "the sidecar comes back");
    let buffer = refused
        .payload
        .expect("a refused write hands its buffer back");
    assert_eq!(
        buffer.as_ptr(),
        address,
        "the same allocation comes back, not a copy"
    );
    assert_eq!(buffer, vec![0xA5_u8; LEN], "and its bytes are untouched");

    batch
        .submit_and_wait(queued, 5_000)
        .expect("submit the filler");
    assert_eq!(
        ring.outstanding(),
        queued as usize,
        "a refused push reserves no identity"
    );
    drain(&mut ring, queued);

    // The retry, with the buffer the refusal handed back.
    let mut batch = Batch::new(&mut ring);
    batch
        .write_owned(
            &file,
            buffer,
            REFUSED,
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
        .expect("the retry is accepted once the queue has room");
    batch.submit_and_wait(1, 5_000).expect("submit the retry");
    let (completion, held) = ring
        .pop_within(POP_BOUND)
        .expect("pop the retry")
        .expect("the retry completes within the bound");
    assert_eq!(
        completion.result().expect("the retried write succeeds"),
        LEN
    );
    let (buffer, extra) = held.expect("the ring held the retried write");
    assert_eq!(extra, REFUSED);
    let buffer = buffer.expect("a write carries its buffer");
    assert_eq!(
        buffer.as_ptr(),
        address,
        "the retry wrote from the buffer the refusal returned"
    );
    assert_eq!(ring.outstanding(), 0);

    drop(file);
    assert_eq!(
        std::fs::read(&path).expect("read the fixture back"),
        vec![0xA5_u8; LEN],
        "the retried write landed"
    );
}

#[test]
fn a_read_refused_by_a_full_queue_hands_back_the_same_buffer_and_sidecar() {
    let (_path, file) = fixture("read");
    let mut ring = Ring::with_inventory(4, 64).expect("create ring");

    let buffer = vec![0x3C_u8; LEN];
    let address = buffer.as_ptr();
    let mut batch = Batch::new(&mut ring);
    let queued = fill(&mut batch, &file);
    let refused = batch
        .read_owned(&file, buffer, REFUSED, 0, PushOptions::new())
        .expect_err("a full queue must refuse the read");
    assert!(refused.error.is_submission_queue_full(), "{refused:?}");
    assert_eq!(refused.extra, REFUSED);
    let buffer = refused
        .payload
        .expect("a refused read hands its buffer back");
    assert_eq!(buffer.as_ptr(), address);
    assert_eq!(
        buffer,
        vec![0x3C_u8; LEN],
        "a refused read wrote nothing into the buffer"
    );

    batch
        .submit_and_wait(queued, 5_000)
        .expect("submit the filler");
    drain(&mut ring, queued);
    assert_eq!(ring.outstanding(), 0);
}

#[test]
fn a_flush_refused_by_a_full_queue_hands_back_its_sidecar() {
    let (_path, file) = fixture("flush");
    let mut ring = Ring::with_inventory(4, 64).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    let queued = fill(&mut batch, &file);
    let refused = batch
        .flush_owned(
            &file,
            REFUSED,
            FlushCoverage::CoversPrecedingOperations,
            FlushMode::Default,
        )
        .expect_err("a full queue must refuse the flush");
    assert!(refused.error.is_submission_queue_full(), "{refused:?}");
    assert_eq!(refused.extra, REFUSED, "the sidecar comes back");
    assert!(refused.payload.is_none(), "a flush takes no payload");

    // A refusal converts to the error it carries, unchanged, so a caller that
    // only wants the error still finds the ring condition (D-30).
    let error = std::io::Error::from(refused);
    assert!(error.is_submission_queue_full());

    batch
        .submit_and_wait(queued, 5_000)
        .expect("submit the filler");
    drain(&mut ring, queued);
}

#[test]
fn a_registered_write_refused_by_a_full_queue_releases_its_use_of_the_registration() {
    let (_path, file) = fixture("registered");
    let mut ring = Ring::with_inventory(4, 64).expect("create ring");

    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_buffers(vec![vec![0x77_u8; LEN]])
        .expect("queue the buffer registration");
    batch.submit_and_wait(1, 5_000).expect("submit and wait");
    let (completion, _held) = ring
        .pop_within(POP_BOUND)
        .expect("pop the registration")
        .expect("the registration completes within the bound");
    let registered = pending
        .claim_if(&completion)
        .expect("the completion is the registration's")
        .expect("the registration succeeded");
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: LEN as u32,
    };

    let mut batch = Batch::new(&mut ring);
    let queued = fill(&mut batch, &file);
    let refused = batch
        .write_registered_owned(
            &file,
            &registered,
            span,
            REFUSED,
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
        .expect_err("a full queue must refuse the registered write");
    assert!(refused.error.is_submission_queue_full(), "{refused:?}");
    assert_eq!(refused.extra, REFUSED, "the sidecar comes back");
    assert!(
        refused.payload.is_none(),
        "the bytes belong to the registration, so there is no payload to return"
    );
    // The use belongs to the ring, not the caller: released, not returned.
    assert_eq!(
        registered.outstanding(0),
        Some(0),
        "a refused push must not leave the registration in use"
    );

    batch
        .submit_and_wait(queued, 5_000)
        .expect("submit the filler");
    drain(&mut ring, queued);

    // The registration is still usable: the same span is accepted on retry.
    let mut batch = Batch::new(&mut ring);
    batch
        .write_registered_owned(
            &file,
            &registered,
            span,
            REFUSED,
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
        .expect("the retry is accepted once the queue has room");
    batch.submit_and_wait(1, 5_000).expect("submit the retry");
    let (completion, _held) = ring
        .pop_within(POP_BOUND)
        .expect("pop the retry")
        .expect("the retry completes within the bound");
    assert_eq!(
        completion.result().expect("the retried write succeeds"),
        LEN
    );
    assert_eq!(registered.outstanding(0), Some(0));
}
