// Copyright (c) 2026 Mike Grier
//! Failures, their reach and their resolution (DI-3.2.4), against real files, with flush failures
//! made on demand by the fault seam. Every entry is reported to the conformance oracle, so each
//! test also checks that `Durable` never passes an unresolved failure and that every suspect write
//! was accepted, in push order.

use windows_ioring_sys::{IoRingErrorExt, RegisteredSpan};

use super::pushes::{ADDED, GIVEN, Harness, Ring, given, instance, temp};
use super::{empty, file, setup};
use crate::contract::{DurableRing, RegisteredBufferRing};
use crate::ids::{FailureId, FailureToken};
use crate::types::{
    Cause, DurabilityRequest, Entry, EpochState, Failed, ImportScope, PushRefusal, Resolution,
    ResolveRefusal,
};

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

/// Resolve, reporting each heal to the oracle.
fn resolve(harness: &mut Harness, items: Vec<(FailureToken, Resolution)>) {
    let healed: Vec<FailureId> = items
        .iter()
        .filter(|(_, resolution)| *resolution == Resolution::Heal)
        .map(|(token, _)| token.id())
        .collect();
    harness.ring.resolve(items).expect("resolve");
    for id in healed {
        harness.oracle.healed(id);
    }
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

#[test]
fn a_failed_flush_is_reported_with_its_cause_and_holds_the_mark() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let failed = failed_seal(&mut harness);
    match &failed.cause {
        Cause::Flush { file, error } => {
            assert_eq!(*file, GIVEN);
            let code = error
                .as_ioring_error()
                .expect("the ring's error, passed through")
                .code();
            assert_eq!(code.cast_unsigned() & 0xFFFF, IO_DEVICE);
        }
        other => panic!("expected a flush failure, got {other:?}"),
    }
    assert_eq!(suspects(&failed), [0, 1]);
    let lineage = harness.ring.default_lineage();
    assert_eq!(harness.ring.durable_through(lineage), None);
    assert_eq!(
        harness.ring.epoch_state(harness.epoch(1)),
        EpochState::Blocked(failed.id)
    );
    assert_eq!(
        harness.seal(1),
        DurabilityRequest::AlreadySealed(EpochState::Blocked(failed.id))
    );
    let inventory = harness.ring.failures();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].id, failed.id);
    assert!(inventory[0].token_live);
    assert!(harness.ring.pop().expect("pop").is_none(), "one answer");
    drop(failed);
    harness.finish();
}

/// CONTRACT.md's "Healing a failure", through the ring.
#[test]
fn a_healed_failure_is_passed_at_the_first_seal_after_the_heal() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let failed = failed_seal(&mut harness);
    let refused = harness
        .ring
        .write(GIVEN, 0, vec![1], harness.epoch(1), 9)
        .expect_err("1 is sealed");
    assert_eq!(refused.context, 9);

    harness.write(GIVEN, 0, b"FIRST", 2, 3);
    harness.write(GIVEN, 8, b"SECOND", 2, 4);
    harness.next_n(2);
    resolve(&mut harness, vec![(failed.token, Resolution::Heal)]);
    assert!(
        harness.ring.take_token(failed.id).is_none(),
        "the heal holds it"
    );
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    for id in [1, 2] {
        match harness.next_entry() {
            Entry::Durable { through } => assert_eq!(through, harness.epoch(id)),
            other => panic!("expected Durable through {id}, got {other:?}"),
        }
    }
    assert!(harness.ring.failures().is_empty());
    assert_eq!(
        harness.ring.epoch_state(harness.epoch(1)),
        EpochState::Durable
    );
    harness.finish();
}

#[test]
fn without_the_heal_a_later_seal_is_answered_blocked() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let failed = failed_seal(&mut harness);
    harness.write(GIVEN, 0, b"again", 2, 3);
    harness.next();
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    match harness.next_entry() {
        Entry::Blocked { through, by } => {
            assert_eq!((through, by), (harness.epoch(2), failed.id));
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    assert_eq!(
        harness.ring.epoch_state(harness.epoch(2)),
        EpochState::Blocked(failed.id)
    );
    resolve(&mut harness, vec![(failed.token, Resolution::Abandon)]);
    match harness.next_entry() {
        Entry::Abandoned { failure, suspect } => {
            assert_eq!(failure, failed.id);
            assert_eq!(suspect.writes().len(), 2);
        }
        other => panic!("expected Abandoned, got {other:?}"),
    }
    let mut durable = Vec::new();
    for _ in 0..2 {
        match harness.next_entry() {
            Entry::Durable { through } => durable.push(through),
            other => panic!("expected Durable, got {other:?}"),
        }
    }
    assert_eq!(durable, [harness.epoch(1), harness.epoch(2)]);
    assert_eq!(
        harness.ring.epoch_state(harness.epoch(1)),
        EpochState::Abandoned
    );
    assert_eq!(
        harness.ring.epoch_state(harness.epoch(2)),
        EpochState::Durable
    );
    harness.finish();
}

#[test]
fn a_failed_flush_reaches_only_files_sharing_a_declared_domain() {
    let log = temp(&[0; 16]);
    let data = temp(&[0; 16]);
    let ring = Ring::new(setup(
        vec![
            file(GIVEN.0, &log, &["disk-1"]),
            file(ADDED.0, &data, &["disk-2"]),
        ],
        Vec::new(),
        None,
    ))
    .expect("an instance with two domains");
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.write(ADDED, 0, b"data", 1, 2);
    harness.next_n(2);
    harness.ring.fail_next_flush(ADDED, IO_DEVICE);
    harness.seal(1);
    let failed = next_failed(&mut harness);
    assert_eq!(suspects(&failed), [1], "the log's disk is not the data's");
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
    .expect("an instance with a declared and an unknown file");
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.write(ADDED, 0, b"data", 1, 2);
    harness.next_n(2);
    let lineage = harness.ring.default_lineage();
    let foreign = empty().default_lineage();
    let cases = [
        (ImportScope::All, vec![0, 1]),
        (ImportScope::Lineage(lineage), vec![0, 1]),
        (ImportScope::Lineage(foreign), vec![]),
        (ImportScope::Domains(vec![super::domain("disk-2")]), vec![1]),
        (
            ImportScope::Domains(vec![super::domain("disk-1")]),
            vec![0, 1],
        ),
    ];
    for (scope, expected) in cases {
        let id = harness.ring.import_failure(scope.clone());
        let failed = next_failed(&mut harness);
        assert_eq!(failed.id, id);
        assert!(matches!(failed.cause, Cause::Imported { scope: ref s } if *s == scope));
        assert_eq!(suspects(&failed), expected, "{scope:?}");
    }
    assert_eq!(harness.ring.failures().len(), 5);
    harness.finish();
}

#[test]
fn a_resolution_naming_another_instances_failure_is_refused_whole() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let ours = failed_seal(&mut harness);
    let mut other = instance(Vec::new(), Vec::new());
    other.import_failure(ImportScope::All);
    let theirs = match other.pop().expect("pop") {
        Some(Entry::Failed(failed)) => failed,
        other => panic!("expected Failed, got {other:?}"),
    };
    let error = harness
        .ring
        .resolve(vec![
            (ours.token, Resolution::Abandon),
            (theirs.token, Resolution::Abandon),
        ])
        .expect_err("a foreign token");
    assert!(matches!(error.reason, ResolveRefusal::Foreign(id) if id == theirs.id));
    assert_eq!(error.returned.len(), 2);
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "nothing was abandoned"
    );
    assert_eq!(harness.ring.failures().len(), 1);
    harness.finish();
}

#[test]
fn a_closed_failure_stays_and_its_token_can_be_taken_again() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let failed = failed_seal(&mut harness);
    assert!(harness.ring.take_token(failed.id).is_none(), "live");
    failed.token.close();
    assert!(!harness.ring.failures()[0].token_live);
    let token = harness.ring.take_token(failed.id).expect("closed");
    resolve(&mut harness, vec![(token, Resolution::Abandon)]);
    assert!(matches!(harness.next_entry(), Entry::Abandoned { .. }));
    assert!(matches!(harness.next_entry(), Entry::Durable { .. }));
    assert!(
        harness.ring.take_token(failed.id).is_none(),
        "resolved, and leaves no memory"
    );
    harness.finish();
}

#[test]
fn a_write_into_an_abandoned_epoch_is_refused_with_what_it_took() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], vec![vec![0; 16]]));
    harness.write(GIVEN, 0, b"sealed", 1, 1);
    harness.write(GIVEN, 8, b"open", 3, 2);
    harness.next_n(2);
    harness.ring.fail_next_flush(GIVEN, IO_DEVICE);
    harness.seal(1);
    let failed = next_failed(&mut harness);
    assert_eq!(suspects(&failed), [0, 1], "sealed 1 and open 3");
    resolve(&mut harness, vec![(failed.token, Resolution::Abandon)]);
    assert!(matches!(harness.next_entry(), Entry::Abandoned { .. }));
    assert!(matches!(harness.next_entry(), Entry::Durable { .. }));

    let three = harness.epoch(3);
    let error = harness
        .ring
        .write(GIVEN, 0, vec![7], three, 5)
        .expect_err("3 was abandoned, though it is open");
    assert!(matches!(error.reason, PushRefusal::EpochAbandoned { epoch } if epoch == three));
    assert_eq!((error.buffer, error.context), (Some(vec![7]), 5));
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let error = harness
        .ring
        .write_registered(GIVEN, 0, span, three, 6)
        .expect_err("a registered write to 3");
    assert!(matches!(error.reason, PushRefusal::EpochAbandoned { .. }));
    assert_eq!((error.buffer, error.context), (None, 6));
    let error = harness
        .ring
        .write(GIVEN, 0, vec![7], harness.epoch(1), 7)
        .expect_err("1 is sealed and abandoned");
    assert!(
        matches!(error.reason, PushRefusal::Sealed { .. }),
        "the seal is checked first: {:?}",
        error.reason
    );
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "a refused write completes nothing"
    );

    harness.write(GIVEN, 0, b"later", 4, 8);
    harness.next();
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
    assert!(matches!(failed.cause, Cause::Flush { file, .. } if file == ADDED));
    drop(failed);
    harness.finish();
}
