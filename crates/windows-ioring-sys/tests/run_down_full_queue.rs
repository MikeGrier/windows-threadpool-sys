// Copyright (c) Mike Grier
//! Rundown over a completion queue that is full (M39.1).
//!
//! # Why this is an integration test
//!
//! It opens a real ring and real completions, an operating-system boundary this repository's Quality
//! rule places in `tests/`.
//!
//! # What this file is for
//!
//! `SubmitIoRing` with a wait reports `IORING_E_COMPLETION_QUEUE_TOO_FULL` when the completion queue
//! holds as many completions as it can, whatever it is waiting for. Rundown waited before it
//! reaped, so a ring dropped with every one of its completions posted and none popped failed its
//! rundown -- and an instance that stops delivering with its queue full has exactly that ring.
//! Reaping first is the rule the completion event's own edge states ([D-19](../DESIGN-NOTES.md)):
//! drain before waiting.

use std::time::Duration;

use windows_ioring_sys::{Batch, FlushCoverage, FlushMode, IoRing, SharedHandle};

mod common;

const QUEUE: u32 = 4;

fn scratch(tag: &str) -> (common::TempPath, SharedHandle) {
    let path = common::TempPath::new("m39-1", tag);
    let file = SharedHandle::new(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .expect("a scratch file")
            .into(),
    );
    (path, file)
}

/// A ring whose completion queue is full, completions unpopped, and an entry still queued behind
/// them because its submit failed for want of room.
fn full(file: &SharedHandle) -> IoRing<Vec<u8>> {
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, QUEUE).expect("a ring");
    let mut pushed = 0;
    loop {
        let mut batch = Batch::new(&mut ring);
        if batch
            .flush_owned(file, (), FlushCoverage::Unordered, FlushMode::Default)
            .is_err()
        {
            break;
        }
        // The push is accepted and its submit then fails, once the kernel has no room for another
        // completion: the entry stays queued, as `SubmitIoRing` documents.
        let submitted = batch.submit();
        pushed += 1;
        if submitted.is_err() {
            break;
        }
        assert!(pushed < 1_000, "the ring never refused");
    }
    assert!(
        pushed > 1,
        "some completed before the queue filled: {pushed}"
    );
    std::thread::sleep(Duration::from_millis(300));
    ring
}

#[test]
fn a_ring_whose_completion_queue_is_full_runs_down() {
    let (_path, file) = scratch("run-down");
    let mut ring = full(&file);
    assert!(ring.outstanding() > 0, "nothing was popped");
    ring.run_down().expect("rundown reaps before it waits");
    assert_eq!(ring.outstanding(), 0);
}

#[test]
fn a_bounded_rundown_of_a_full_completion_queue_finishes() {
    let (_path, file) = scratch("run-down-within");
    let mut ring = full(&file);
    assert!(
        ring.run_down_within(Duration::from_secs(5))
            .expect("rundown reaps before it waits"),
        "and finishes inside its bound"
    );
}

#[test]
fn a_ring_whose_completion_queue_is_full_drops_cleanly() {
    let (_path, file) = scratch("drop");
    drop(full(&file));
}
