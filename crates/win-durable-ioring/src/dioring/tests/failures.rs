// Copyright (c) 2026 Mike Grier
//! Failures against real files (DI-3.2.4): what the Windows driver adds to them -- a flush's failure
//! relayed from the ring, and the fault seam itself -- and imports. How the core answers a failed
//! flush is tested in `durable-ioring`, over its in-memory backend's script (DI-18.2). Every entry is
//! reported to the conformance oracle, so each test also checks that `Durable` never passes an
//! unresolved failure and that every suspect write was accepted, in push order.

use windows_ioring_sys::IoRingErrorExt;

use super::pushes::{ADDED, GIVEN, Harness, Ring, given, instance, temp};
use super::{empty, file, setup};
use durable_ioring::DurableRing;
use durable_ioring::{DurabilityRequest, Entry, Failed, FailureCause, ImportScope, UnknownLineage};

/// `ERROR_IO_DEVICE`, the failure the seam is armed with.
const IO_DEVICE: u32 = 1117;

type V = super::V;

/// The next entry, which must be `Failed`.
fn next_failed(harness: &mut Harness) -> Failed<V> {
    match harness.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// The ops a failure suspects, by sequence number.
fn suspects(failed: &Failed<V>) -> Vec<u64> {
    failed.suspect.writes().iter().map(|w| w.op.seq).collect()
}

/// Two writes tagged 1 to `GIVEN`, completed, then sealed through 1 with its flush failing.
fn failed_seal(harness: &mut Harness) -> Failed<V> {
    harness.write(GIVEN, 0, b"first", 1, 1);
    harness.write(GIVEN, 8, b"second", 1, 2);
    harness.next_n(2);
    harness.ring.fail_next_flush(GIVEN, IO_DEVICE);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    next_failed(harness)
}

/// The Windows half of a test whose core half -- the suspects, the mark held, the inventory -- moved to
/// `durable-ioring` (DI-18.2): the failure's cause is the ring's, relayed.
#[test]
fn a_failed_flush_is_reported_with_the_rings_cause() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let failed = failed_seal(&mut harness);
    match &failed.cause {
        FailureCause::Commit {
            target: file,
            fault,
        } => {
            assert_eq!(*file, GIVEN);
            assert_eq!(fault.source, "windows-ioring", "relayed from the ring");
            let code = fault
                .error
                .downcast_ref::<std::io::Error>()
                .and_then(|error| error.as_ioring_error())
                .expect("the ring's error, passed through")
                .code();
            assert_eq!(code.cast_unsigned() & 0xFFFF, IO_DEVICE);
        }
        other => panic!("expected a flush failure, got {other:?}"),
    }
    drop(failed);
    harness.finish();
}

#[test]
fn an_import_suspects_what_its_scope_reaches() {
    let log = temp(&[0; 16]);
    let data = temp(&[0; 16]);
    let ring = Ring::new(setup(
        vec![file(GIVEN.0, &log, &["disk-1"]), file(ADDED.0, &data, &[])],
        Vec::new(),
        None,
    ))
    .expect("an instance with a declared and an unknown file")
    .0;
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.write(ADDED, 0, b"data", 1, 2);
    harness.next_n(2);
    let lineage = harness.lineage();
    let foreign = empty().default_lineage().lineage();
    assert_eq!(
        harness.ring.import_failure(ImportScope::Lineage(foreign)),
        Err(UnknownLineage(foreign)),
        "a lineage the instance cannot answer for is refused, and nothing is recorded"
    );
    let disk_2 = super::domain("disk-2");
    let cases = [
        (ImportScope::All, vec![0, 1], vec![]),
        (ImportScope::Lineage(lineage), vec![0, 1], vec![]),
        (
            ImportScope::Domains(vec![disk_2.clone()]),
            vec![1],
            vec![disk_2],
        ),
        (
            ImportScope::Domains(vec![super::domain("disk-1")]),
            vec![0, 1],
            vec![],
        ),
    ];
    for (scope, expected, unmatched) in cases {
        let imported = harness.ring.import_failure(scope.clone()).expect("import");
        assert_eq!(imported.unmatched, unmatched, "{scope:?}");
        let failed = next_failed(&mut harness);
        assert_eq!(failed.id, imported.failure);
        assert!(matches!(failed.cause, FailureCause::Imported { scope: ref s } if *s == scope));
        assert_eq!(suspects(&failed), expected, "{scope:?}");
    }
    assert_eq!(harness.ring.failures().len(), 4);
    harness.finish();
}

/// A name the instance has never been given reaches only the files declared with none -- right
/// when declarations are complete -- and is reported, so a misspelling is not silent (DI-D-46).
#[test]
fn an_imported_domain_the_instance_was_never_given_is_reported_and_reaches_the_unknown_files() {
    let (log, data) = (temp(&[0; 16]), temp(&[0; 16]));
    let ring = Ring::new(setup(
        vec![file(GIVEN.0, &log, &["disk-1"]), file(ADDED.0, &data, &[])],
        Vec::new(),
        None,
    ))
    .expect("an instance with a declared and an unknown file")
    .0;
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.write(ADDED, 0, b"data", 1, 2);
    harness.next_n(2);
    let named = vec![
        super::domain("disk-l"),
        super::domain("disk-1"),
        super::domain("disk-9"),
    ];
    let imported = harness
        .ring
        .import_failure(ImportScope::Domains(named))
        .expect("import");
    assert_eq!(
        imported.unmatched,
        [super::domain("disk-l"), super::domain("disk-9")],
        "in the order named; disk-1 matched"
    );
    assert_eq!(suspects(&next_failed(&mut harness)), [0, 1]);

    let only_unknown = harness
        .ring
        .import_failure(ImportScope::Domains(vec![super::domain("disk-l")]))
        .expect("import");
    assert_eq!(only_unknown.unmatched, [super::domain("disk-l")]);
    assert_eq!(
        suspects(&next_failed(&mut harness)),
        [1],
        "only the file declared with none"
    );
    harness.finish();
}

#[test]
fn an_armed_failure_is_used_by_its_own_file_only() {
    let first = temp(&[0; 16]);
    let second = temp(&[0; 16]);
    let mut harness = Harness::new(instance(
        vec![given(GIVEN, &first), given(ADDED, &second)],
        Vec::new(),
    ));
    harness.ring.fail_next_flush(ADDED, IO_DEVICE);
    harness.write(GIVEN, 0, b"kept", 1, 1);
    harness.next();
    harness.seal(1);
    match harness.next_entry() {
        Entry::Durable { through } => assert_eq!(through, harness.epoch(1)),
        other => panic!("GIVEN's flush was not armed: {other:?}"),
    }
    harness.write(ADDED, 0, b"lost", 2, 2);
    harness.next();
    harness.seal(2);
    let failed = next_failed(&mut harness);
    assert!(matches!(failed.cause, FailureCause::Commit { target, .. } if target == ADDED));
    drop(failed);
    harness.finish();
}
