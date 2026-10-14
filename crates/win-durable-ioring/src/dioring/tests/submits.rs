// Copyright (c) 2026 Mike Grier
//! Failed submissions (DI-7.1): what the driver does when the ring will not submit, through a
//! responder registered on the instance's own ring.
//!
//! The ring crate's responder (its D-85) answers a ring's calls on whatever thread makes them, so
//! the submits the pool threads make fail too, and a drain's real submit cannot undo the failure.
//! It is the ring crate's safe `sys::submit_faults`: this crate forbids `unsafe` (DI-D-4), and
//! implementing the seam's trait needs it.
//!
//! What fails is a submit, not a push: a push is accepted, its work left queued in the ring, and
//! the submission is owed. Every test clears the fault before its instance ends, because the ring
//! cannot run its operations down while its submits fail.

use std::time::{Duration, Instant};

use windows_ioring_sys::InjectedFailure;
use windows_ioring_sys::sys::Call;

use super::gates::expect_durable;
use super::pumped::{next as pumped_next, pumped, readiness_set};
use super::pushes::{BOUND, GIVEN, Harness, RingEntry, given, instance, temp};
use durable_ioring::DurableRing;
use durable_ioring::backend::{RefusalCause, SubmissionData};
use durable_ioring::{DurabilityRequest, Entry, OpKind, ReadOptions};

/// `ERROR_IO_DEVICE`: no condition the ring names, so a failed submit of unknown cause.
fn device() -> InjectedFailure {
    InjectedFailure::Win32(1117)
}

/// The cause a failed `pop` reported, recovered from its error.
fn cause_of(error: &std::io::Error) -> &RefusalCause {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<RefusalCause>())
        .expect("a pop's failed submit is reported as its RefusalCause")
}

/// Whether `cause` is the failure `device` arms.
fn is_the_armed_failure(cause: &RefusalCause) -> bool {
    matches!(
        cause,
        RefusalCause::Unknown(fault) if fault.code.and_then(crate::ErrorCode::win32) == Some(1117)
    )
}

/// The next entry, within the bound, popping through the failures a failing submit makes `pop`
/// report: a pop with an entry to return returns it, and without one reports the submit.
fn pop_through_failures(harness: &mut Harness) -> RingEntry {
    let started = Instant::now();
    loop {
        if let Ok(Some(entry)) = harness.ring.pop() {
            harness
                .oracle
                .observe(&entry)
                .expect("the oracle accepts the entry");
            return entry;
        }
        assert!(
            started.elapsed() < BOUND,
            "no entry within {BOUND:?}; {} outstanding",
            harness.oracle.outstanding()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_pop_whose_submit_fails_reports_its_cause_and_the_work_is_issued_once_submits_succeed() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let faults = harness.ring.engine().backend.submit_faults();
    faults.fail_until_cleared(device());

    // Accepted, though its own submit failed: the work is queued in the ring, and owed.
    let id = harness.write(GIVEN, 0, b"queued", 1, 7);

    let error = harness
        .ring
        .pop()
        .expect_err("nothing to return, and the submit that is owed fails");
    assert!(
        is_the_armed_failure(cause_of(&error)),
        "reported with its cause: {error}"
    );
    let again = harness.ring.pop().expect_err("a report is not consumed");
    assert!(is_the_armed_failure(cause_of(&again)));

    faults.clear();
    let done = harness.next();
    assert_eq!(done.id, id, "the work was issued, and completed");
    assert_eq!(done.context, 7);
    assert_eq!(super::pushes::transferred(&done), 6);
    assert_eq!(
        std::fs::read(&temp.0).expect("read the file")[..6],
        *b"queued",
        "and it reached the file"
    );
    harness.finish();
}

#[test]
fn a_drain_whose_submit_fails_on_a_pool_thread_leaves_the_work_queued_for_the_next_submit() {
    let temp = temp(&[7; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let faults = harness.ring.engine().backend.submit_faults();
    let here = std::thread::current().id();

    let (first, second) = {
        // Held, so no pool drain can run until both reads are pushed. The drain the first read's
        // completion calls for then starts with a submit that is owed because of the second, and
        // fails -- on a pool thread, which is what a thread-local responder could not reach.
        let mut engine = harness.ring.engine();
        // What the driver's own read does where its buffer type allows (DI-D-17); a push through
        // the engine directly does not.
        engine.backend.enable_owned_reads();
        let first = engine
            .read(
                GIVEN,
                0,
                SubmissionData::Owned(vec![0; 4]),
                1,
                ReadOptions::new(),
            )
            .expect("accepted");
        faults.fail_until_cleared(device());
        let second = engine
            .read(
                GIVEN,
                4,
                SubmissionData::Owned(vec![0; 4]),
                2,
                ReadOptions::new(),
            )
            .expect("accepted, though its own submit fails");
        (first, second)
    };
    harness.pushed(first, OpKind::Read, 4, 1);
    harness.pushed(second, OpKind::Read, 4, 2);

    // Only a pool drain pops a completion from the ring, so this entry's arrival is that drain's
    // work, and the drain begins with a submit. A consumer's pop prefers an entry to the report.
    let Entry::Op(done) = pop_through_failures(&mut harness) else {
        panic!("the first read's completion");
    };
    assert_eq!(done.id, first);
    let threads = faults.failures();
    assert!(
        threads.iter().any(|thread| *thread != here),
        "a submit failed on a thread other than the test's: {threads:?}"
    );

    // The second read is queued, not lost: nothing completes for it while submits fail, and a pop
    // says why.
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(100) {
        let error = harness
            .ring
            .pop()
            .expect_err("nothing to return, and a submit is owed");
        assert!(is_the_armed_failure(cause_of(&error)));
        std::thread::sleep(Duration::from_millis(1));
    }

    faults.clear();
    let done = harness.next();
    assert_eq!(done.id, second, "the next submit issued it");
    assert_eq!(done.context, 2);
    harness.finish();
}

#[test]
fn a_seal_whose_commit_submit_fails_is_made_durable_once_submits_succeed() {
    // A submission of the core's own, rather than the consumer's: the commit a seal calls for is
    // pushed by the engine, and its failed submit is owed just as a write's is.
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let faults = harness.ring.engine().backend.submit_faults();

    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    faults.fail_until_cleared(device());
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);

    let error = harness
        .ring
        .pop()
        .expect_err("the commit is queued, not issued, and its submit fails");
    assert!(is_the_armed_failure(cause_of(&error)));
    assert_eq!(
        harness.durable(),
        None,
        "not durable while the commit has not run"
    );

    faults.clear();
    let one = harness.epoch(1);
    expect_durable(&mut harness, one);
    assert_eq!(harness.durable(), Some(1));
    harness.finish();
}

/// The fault a pop reported, recovered from its error.
fn fault_of(error: &std::io::Error) -> &crate::IoFault {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<crate::IoFault>())
        .expect("a pop's unreadable ring is reported as its IoFault")
}

/// Long enough for a wake's two drains to have run, one before the ring's wait is armed again and
/// one after.
const SETTLED: Duration = Duration::from_millis(250);

#[test]
fn a_write_whose_doorbell_fails_sets_the_readiness_signal_once_and_the_pop_reports_the_cause() {
    // DI-7+.2. Pumped, so nothing but the failed doorbell can have set the signal. The consumer
    // waits on the signal and pops, which rings the doorbell again and says why it cannot.
    let temp = temp(&[0; 64]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    let faults = harness.ring.engine().backend.submit_faults();
    assert!(!readiness_set(&mut harness), "nothing has set it");

    faults.fail_until_cleared(device());
    let id = harness.write(GIVEN, 0, b"queued", 1, 7);
    assert!(
        readiness_set(&mut harness),
        "the failed doorbell told the consumer"
    );
    assert!(!readiness_set(&mut harness), "once");
    let error = harness.ring.pop().expect_err("the doorbell fails again");
    assert!(is_the_armed_failure(cause_of(&error)));

    faults.clear();
    assert!(
        harness.ring.pop().expect("the doorbell works").is_none(),
        "the pop's own retry issued it, and nothing has completed yet"
    );
    let Entry::Op(done) = pumped_next(&mut harness) else {
        panic!("the write's completion");
    };
    assert_eq!(done.id, id);
    harness.finish();
}

#[test]
fn a_ring_that_cannot_be_read_sets_the_readiness_signal_and_the_pop_reports_it_once() {
    // DI-7+.2. Both of a wake's drains fail, and the first fault is the one kept.
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let faults = harness.ring.engine().backend.call_faults();
    faults.fail_until_cleared(Call::Pop, device());

    harness.write(GIVEN, 0, b"queued", 1, 7);
    std::thread::sleep(SETTLED);
    assert!(
        readiness_set(&mut harness),
        "the drain that could not read told the consumer"
    );
    let error = harness.ring.pop().expect_err("nothing to return, and why");
    assert_eq!(
        fault_of(&error).code.and_then(crate::ErrorCode::win32),
        Some(1117)
    );
    assert!(
        harness.ring.pop().expect("nothing more to say").is_none(),
        "reported once"
    );

    // A pool instance has no next wake to read it: the ring's event is an edge, taken by the wake
    // that could not read, and the completion it left keeps the queue from going from empty to
    // non-empty again. The consumer's remedy is to drop the instance, which is what ends this test.
    faults.clear();
}

#[test]
fn a_pumped_instance_whose_ring_could_not_be_read_reads_it_at_the_next_pump() {
    // DI-7+.2. A pump drains before it waits, so what a failed read left is there for the next.
    let temp = temp(&[0; 64]);
    let mut harness = pumped(vec![given(GIVEN, &temp)]);
    let faults = harness.ring.engine().backend.call_faults();
    let id = harness.write(GIVEN, 0, b"queued", 1, 7);
    std::thread::sleep(SETTLED);
    faults.fail_next(Call::Pop, 1, device());

    assert!(
        harness.ring.pump(Some(BOUND)).expect("pump"),
        "told, so the consumer pops"
    );
    let error = harness
        .ring
        .pop()
        .expect_err("the unreadable ring, reported");
    assert_eq!(
        fault_of(&error).code.and_then(crate::ErrorCode::win32),
        Some(1117)
    );
    let Entry::Op(done) = pumped_next(&mut harness) else {
        panic!("the write's completion");
    };
    assert_eq!(done.id, id, "read once the ring could be");
    harness.finish();
}
#[test]
fn a_fault_the_next_read_got_past_is_not_reported_to_the_consumer() {
    // DI-7+.2. The first of a wake's drains fails and the second, after the wait is armed again,
    // reads: the fault is stale by the time the consumer pops, which returns the entry and then
    // nothing, not the fault.
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let faults = harness.ring.engine().backend.call_faults();
    faults.fail_next(Call::Pop, 1, device());

    let id = harness.write(GIVEN, 0, b"queued", 1, 7);
    std::thread::sleep(SETTLED);
    let done = harness.next();
    assert_eq!(done.id, id);
    assert!(
        harness.ring.pop().expect("no stale report").is_none(),
        "nothing left, and no fault"
    );
    harness.finish();
}
