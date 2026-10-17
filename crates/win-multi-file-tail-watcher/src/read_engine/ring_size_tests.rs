// Copyright (c) 2026 Mike Grier
//! What the ring does with the two queue sizes it is asked for, and how its completion queue bounds
//! the reads the engine has in flight at once against the pool's buffer count (TW-9.1).
//!
//! The rules asserted here were observed on the host recorded in
//! [DESIGN-NOTES.md](../../DESIGN-NOTES.md) under TW-D-16. Limits are asserted against what
//! `capabilities()` reports, not against that host's numbers.

use windows_ioring_sys::{IoRing, IoRingErrorExt, RingCondition, RingInfo, capabilities};

use super::{Advance, EngineError, ReadEngine, Stop};
use crate::Watcher;
use crate::pool::BufferPool;
use crate::reader::TailReaderOptions;
use crate::test_support::{TempDir, wtf};

type Ring = IoRing<(), ()>;

/// The submission queue size a request of zero is replaced by.
const DEFAULT_SQ: u32 = 32;
/// The least a nonzero request gets, and the ratio of the completion queue to the submission queue:
/// the reading layer's options refuse what the ring would raise, using these two.
const LEAST_SQ: u32 = TailReaderOptions::MIN_RING_SQ_SIZE;
const CQ_PER_SQ: u32 = TailReaderOptions::MIN_RING_CQ_PER_SQ;

fn reported(sq: u32, cq: u32) -> RingInfo {
    Ring::with_inventory(sq, cq)
        .unwrap_or_else(|error| panic!("a ring of {sq} and {cq}: {error}"))
        .info()
        .expect("a ring reports its sizes")
}

/// The condition a refused request names.
fn refusal(sq: u32, cq: u32) -> Option<RingCondition> {
    let Err(error) = Ring::with_inventory(sq, cq) else {
        return None;
    };
    let code = error.as_ioring_error()?.code();
    RingCondition::from_hresult(code)
}

/// The submission queue size the ring gives a request: zero is the default, anything else is
/// rounded up to a power of two of at least eight.
fn expected_sq(request: u32) -> u32 {
    if request == 0 {
        DEFAULT_SQ
    } else {
        request.next_power_of_two().max(LEAST_SQ)
    }
}

/// The completion queue size the ring gives a request: the larger of twice the submission queue
/// it settled on and the request rounded up to a power of two.
fn expected_cq(sq: u32, cq_request: u32) -> u32 {
    (expected_sq(sq) * CQ_PER_SQ).max(cq_request.next_power_of_two())
}

#[test]
fn a_submission_queue_is_rounded_up_to_a_power_of_two_of_at_least_eight() {
    for request in (0..=70).chain([100, 127, 128, 129, 1000, 1024, 1025, 4096, 10_000, 65_536]) {
        assert_eq!(
            reported(request, 0).submission_queue_size,
            expected_sq(request),
            "a submission queue of {request}"
        );
    }
}

#[test]
fn a_completion_queue_is_the_larger_of_twice_the_submission_queue_and_the_request_rounded_up() {
    let submission = [
        0, 1, 7, 8, 9, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 1000, 1024, 1025, 65_536,
    ];
    let completion = [
        0, 1, 15, 16, 17, 64, 65, 128, 129, 1000, 65_536, 65_537, 131_072,
    ];
    for sq in submission {
        for cq in completion {
            let info = reported(sq, cq);
            assert_eq!(
                info.submission_queue_size,
                expected_sq(sq),
                "the submission queue of a ring of {sq} and {cq}"
            );
            assert_eq!(
                info.completion_queue_size,
                expected_cq(sq, cq),
                "the completion queue of a ring of {sq} and {cq}"
            );
        }
    }
}

#[test]
fn a_size_of_zero_is_accepted_and_replaced_by_a_default_the_caller_did_not_choose() {
    for (sq, cq) in [(0, 0), (0, 1), (1, 0)] {
        let info = reported(sq, cq);
        assert_eq!(
            info.submission_queue_size,
            expected_sq(sq),
            "the submission queue of a ring of {sq} and {cq}"
        );
    }
    let zero = reported(0, 0);
    assert_eq!(
        (zero.submission_queue_size, zero.completion_queue_size),
        (DEFAULT_SQ, DEFAULT_SQ * CQ_PER_SQ)
    );
}

#[test]
fn the_largest_sizes_are_the_ones_the_ring_reports_and_one_more_is_refused() {
    let caps = capabilities().expect("the ring's capabilities");
    let (max_sq, max_cq) = (
        caps.max_submission_queue_size,
        caps.max_completion_queue_size,
    );

    assert_eq!(reported(max_sq, 0).submission_queue_size, max_sq);
    assert_eq!(reported(max_sq - 1, 0).submission_queue_size, max_sq);
    assert_eq!(
        refusal(max_sq + 1, 64),
        Some(RingCondition::SubmissionQueueTooBig)
    );
    assert_eq!(
        refusal(u32::MAX, 64),
        Some(RingCondition::SubmissionQueueTooBig)
    );

    assert_eq!(reported(64, max_cq).completion_queue_size, max_cq);
    assert_eq!(
        reported(64, max_cq / 2 + 1).completion_queue_size,
        max_cq,
        "a request past half the largest rounds up to it"
    );
    assert_eq!(
        refusal(64, max_cq + 1),
        Some(RingCondition::CompletionQueueTooBig)
    );
    assert_eq!(
        refusal(64, u32::MAX),
        Some(RingCondition::CompletionQueueTooBig)
    );

    // Both too big: the submission queue is the one that is named.
    assert_eq!(
        refusal(max_sq + 1, max_cq + 1),
        Some(RingCondition::SubmissionQueueTooBig)
    );
}

fn engine_of(
    buffer_len: usize,
    buffers: usize,
    ring_sq: u32,
    ring_cq: u32,
) -> Result<ReadEngine<u32>, EngineError> {
    let watcher = Watcher::new(1024).expect("a watcher");
    let pool = BufferPool::new(buffer_len, buffers).expect("a pool");
    ReadEngine::new(watcher, pool, 256, ring_sq, ring_cq)
}

#[test]
fn an_engine_at_the_largest_ring_works_and_one_over_is_refused_with_the_rings_condition() {
    let caps = capabilities().expect("the ring's capabilities");
    let (max_sq, max_cq) = (
        caps.max_submission_queue_size,
        caps.max_completion_queue_size,
    );
    let largest = engine_of(16, 4, max_sq, max_cq).expect("an engine at the largest ring");
    assert_eq!(
        largest.cq_size,
        usize::try_from(reported(max_sq, max_cq).completion_queue_size).expect("a u32 fits usize"),
        "the engine bounds the reads by the size the ring reports"
    );

    for (sq, cq, condition) in [
        (max_sq + 1, 64, RingCondition::SubmissionQueueTooBig),
        (64, max_cq + 1, RingCondition::CompletionQueueTooBig),
    ] {
        let Err(EngineError::Ring(error)) = engine_of(16, 4, sq, cq) else {
            panic!("a ring of {sq} and {cq} cannot be made");
        };
        let code = error
            .as_ioring_error()
            .expect("the ring's own error")
            .code();
        assert_eq!(RingCondition::from_hresult(code), Some(condition));
    }
}

/// How many reads an engine holds after planning one attention of `pieces` buffers and submitting
/// as many as it can, before any completion is popped, and why it stopped.
fn submitted(
    pieces: usize,
    buffers: usize,
    ring_sq: u32,
    ring_cq: u32,
) -> (usize, Option<Stop>, usize) {
    const BUFFER: usize = 16;
    let dir = TempDir::new("re-bound");
    let path = dir.file("a.log", &vec![b'x'; pieces * BUFFER]);
    let mut engine = engine_of(BUFFER, buffers, ring_sq, ring_cq).expect("an engine");
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    assert!(
        matches!(engine.advance().expect("plan"), Advance::Progress),
        "the first step plans the attention"
    );
    let stop = match engine.advance().expect("submit") {
        Advance::Progress => None,
        Advance::Blocked(stop) => Some(stop),
        Advance::Drained => panic!("something was planned"),
    };
    (engine.in_flight(), stop, engine.cq_size)
}

#[test]
fn reads_in_flight_are_the_least_of_the_pieces_the_buffers_and_the_completion_queue() {
    // (pieces, buffers, ring sq, ring cq) -> (in flight, why it stopped)
    let cases = [
        // The pool is the limit: 64 buffers under a completion queue of 128.
        (200, 64, 64, 64, 64, Some(Stop::PoolEmpty)),
        // The completion queue is the limit: 16 under a pool of 300.
        (200, 300, 8, 8, 16, Some(Stop::RingFull)),
        // Equal: the completion queue is checked first.
        (200, 16, 8, 8, 16, Some(Stop::RingFull)),
        // The pool is the smaller.
        (200, 10, 8, 8, 10, Some(Stop::PoolEmpty)),
        // Everything planned fits.
        (5, 64, 64, 64, 5, None),
        // The submission queue is not the limit: 64 of it, 128 reads in flight.
        (200, 300, 64, 16, 128, Some(Stop::RingFull)),
        (200, 300, 8, 64, 64, Some(Stop::RingFull)),
    ];
    for (pieces, buffers, sq, cq, in_flight, stop) in cases {
        let (held, why, completion_queue) = submitted(pieces, buffers, sq, cq);
        let rounded = usize::try_from(expected_cq(sq, cq)).expect("a u32 fits usize");
        assert_eq!(
            completion_queue, rounded,
            "the completion queue of a ring of {sq} and {cq}"
        );
        assert_eq!(
            held, in_flight,
            "{pieces} pieces, {buffers} buffers, a ring of {sq} and {cq}"
        );
        assert_eq!(
            held,
            pieces.min(buffers).min(completion_queue),
            "the least of the pieces, the buffers and the completion queue"
        );
        assert_eq!(
            why, stop,
            "{pieces} pieces, {buffers} buffers, a ring of {sq} and {cq}"
        );
    }
}
