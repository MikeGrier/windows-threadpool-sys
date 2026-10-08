// Copyright (c) 2026 Mike Grier
//! Markings and a failure's final record (DI-3.2.4.2) through a real instance, every entry checked
//! by the conformance oracle. A nullifier cannot be made here on demand -- its write would have to
//! be in flight when the failure is observed, which a test cannot hold it to -- so the core's
//! tests cover it.

use crate::contract::DurableRing;
use crate::error_code::ErrorCode;
use crate::ids::FailureId;
use crate::types::{
    Cause, DurabilityRequest, Entry, Failed, ImportScope, Marking, MarkingKind, Resolution,
};

use super::pushes::{GIVEN, Harness, given, instance, temp};

/// `ERROR_IO_DEVICE`, the failure the seam is armed with.
const IO_DEVICE: u32 = 1117;

type V = super::V;

fn next_failed(harness: &mut Harness) -> Failed<V> {
    match harness.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    }
}

fn next_marked(harness: &mut Harness) -> (FailureId, Marking<V>) {
    match harness.next_entry() {
        Entry::Marked { failure, marking } => (failure, marking),
        other => panic!("expected Marked, got {other:?}"),
    }
}

#[test]
fn a_suspect_write_flushed_after_an_import_is_marked_covered_and_abandoning_carries_it() {
    let file = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &file)], Vec::new()));
    let write = harness.write(GIVEN, 0, b"covered", 2, 1);
    harness.next();
    harness.ring.import_failure(ImportScope::All);
    let failed = next_failed(&mut harness);
    assert_eq!(failed.suspect.writes()[0].op, write);

    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    let (failure, marking) = next_marked(&mut harness);
    assert_eq!((failure, marking.write), (failed.id, write));
    assert_eq!(marking.kind, MarkingKind::Covered);
    assert!(
        matches!(harness.next_entry(), Entry::Blocked { by, .. } if by == failed.id),
        "covered, and still held: a marking changes nothing the mark depends on"
    );
    assert_eq!(
        harness.ring.failures()[0].markings,
        std::slice::from_ref(&marking)
    );

    harness
        .ring
        .resolve(vec![(failed.token, Resolution::Abandon)])
        .expect("abandon");
    match harness.next_entry() {
        Entry::Abandoned { markings, .. } => assert_eq!(markings, [marking]),
        other => panic!("expected Abandoned, got {other:?}"),
    }
    assert!(matches!(harness.next_entry(), Entry::Durable { .. }));
    harness.finish();
}

#[test]
fn a_heal_reports_its_record_with_the_markings_its_failure_gained() {
    let file = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &file)], Vec::new()));
    let sealed = harness.write(GIVEN, 0, b"first", 1, 1);
    let later = harness.write(GIVEN, 8, b"second", 2, 2);
    harness.next_n(2);
    harness.ring.fail_next_flush(GIVEN, IO_DEVICE);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let failed = next_failed(&mut harness);
    let suspects: Vec<_> = failed.suspect.writes().iter().map(|w| w.op).collect();
    assert_eq!(suspects, [sealed, later]);

    harness
        .ring
        .resolve(vec![(failed.token, Resolution::Heal)])
        .expect("heal");
    harness.oracle.healed(failed.id);
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    let (_, covered) = next_marked(&mut harness);
    assert_eq!(
        (covered.write, covered.kind),
        (later, MarkingKind::Covered),
        "a healing failure goes on gaining markings"
    );
    match harness.next_entry() {
        Entry::Healed {
            failure, markings, ..
        } => {
            assert_eq!(failure, failed.id);
            assert_eq!(markings, [covered]);
        }
        other => panic!("expected Healed, got {other:?}"),
    }
    for id in [1, 2] {
        match harness.next_entry() {
            Entry::Durable { through } => assert_eq!(through, harness.epoch(id)),
            other => panic!("expected Durable through {id}, got {other:?}"),
        }
    }
    harness.finish();
}

#[test]
fn a_failed_flushs_code_reads_without_the_ring_crate() {
    let file = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &file)], Vec::new()));
    harness.write(GIVEN, 0, b"x", 1, 1);
    harness.next();
    harness.ring.fail_next_flush(GIVEN, IO_DEVICE);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    match next_failed(&mut harness).cause {
        Cause::Flush { error, .. } => {
            let code = ErrorCode::of(&error).expect("a flush the kernel answered carries a code");
            assert_eq!(code.win32(), Some(IO_DEVICE));
        }
        other => panic!("expected a flush failure, got {other:?}"),
    }
    harness.finish();
}
