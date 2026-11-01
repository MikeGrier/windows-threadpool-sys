// Copyright (c) 2026 Mike Grier
//! Markings and a failure's final record (DI-3.2.4.2) through a real instance, every entry checked
//! by the conformance oracle. A nullifier cannot be made here on demand -- its write would have to
//! be in flight when the failure is observed, which a test cannot hold it to -- so the core's
//! tests cover it.

use durable_ioring::DurableRing;
use durable_ioring::{
    DurabilityRequest, Entry, Failed, FailureCause, ImportScope, MarkingKind, Resolution,
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

#[test]
fn a_suspect_write_flushed_after_an_import_is_marked_covered_and_the_observer_is_told() {
    let file = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &file)], Vec::new()));
    let write = harness.write(GIVEN, 0, b"covered", 2, 1);
    harness.next();
    harness
        .ring
        .import_failure(ImportScope::All, |_| {})
        .expect("import");
    let failed = next_failed(&mut harness);
    assert_eq!(harness.suspects(failed.id), [write]);
    assert_eq!(harness.told.markings(failed.id), Some(Vec::new()));

    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    assert!(
        matches!(harness.next_entry(), Entry::Blocked { by, .. } if by == failed.id),
        "covered, and still held: a marking changes nothing the mark depends on"
    );
    let markings = harness.told.markings(failed.id).expect("told of it");
    assert_eq!(markings.len(), 1);
    assert_eq!(markings[0].write, write);
    assert_eq!(
        markings[0].kind,
        MarkingKind::Covered {
            flushed: true,
            provider: false
        }
    );

    harness
        .ring
        .resolve(vec![(failed.token, Resolution::Abandon)])
        .expect("abandon");
    match harness.next_entry() {
        Entry::Abandoned { failure } => assert_eq!(failure, failed.id),
        other => panic!("expected Abandoned, got {other:?}"),
    }
    assert!(matches!(harness.next_entry(), Entry::Durable { .. }));
    assert_eq!(
        harness.told.markings(failed.id).as_deref(),
        Some(markings.as_slice()),
        "resolved, and told no more"
    );
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
        FailureCause::Commit { fault, .. } => {
            let code = fault
                .code
                .expect("a commit the ring answered carries a code");
            assert_eq!(crate::win32_code_of(code), Some(IO_DEVICE));
            assert_eq!(
                crate::error_code_of(
                    fault
                        .error
                        .downcast_ref::<std::io::Error>()
                        .expect("the ring's io::Error")
                ),
                Some(code),
                "read again from the report"
            );
        }
        other => panic!("expected a flush failure, got {other:?}"),
    }
    harness.finish();
}
