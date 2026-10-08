// Copyright (c) 2026 Mike Grier
//! Seals and durability in the default lineage (DI-3.2.3), against real files.
//!
//! Every seal the instance accepts is reported to the conformance oracle, so each test also checks
//! that `Durable` is reported only for what was sealed and only after the writes it covers
//! (guarantee 5), without restating those rules.

use std::io;

use windows_ioring_sys::RegisteredSpan;

use super::pushes::{ADDED, GIVEN, Harness, Ring, given, instance, read_only, temp};
use super::{empty, file, serves, setup};
use crate::contract::{DurableRing, RegisteredBufferRing};
use crate::types::{
    DurabilityRequest, Entry, Epoch, EpochState, Outcome, PushRefusal, UnknownLineage,
};

/// The next entry, which must be `Durable` through `id` of the default lineage.
fn expect_durable(harness: &mut Harness, id: u64) {
    match harness.next_entry() {
        Entry::Durable { through } => assert_eq!(through, harness.epoch(id)),
        other => panic!("expected Durable through {id}, got {other:?}"),
    }
}

#[test]
fn a_seal_after_its_writes_completed_is_reported_durable() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    assert_eq!(harness.state(1), EpochState::Open);
    assert_eq!(harness.sealed(), None);

    harness.write(GIVEN, 0, b"first", 1, 1);
    harness.write(GIVEN, 8, b"second", 1, 2);
    harness.next_n(2);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    assert_eq!(harness.sealed(), Some(1));
    expect_durable(&mut harness, 1);

    assert_eq!(harness.durable(), Some(1));
    assert_eq!(harness.state(1), EpochState::Durable);
    assert_eq!(harness.state(2), EpochState::Open);
    let lineages = harness.ring.lineages();
    assert_eq!(lineages.len(), 1);
    assert_eq!(
        (lineages[0].sealed_through, lineages[0].durable_through),
        (Some(1), Some(1))
    );
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "one Durable per seal"
    );
    harness.finish();
}

#[test]
fn a_seal_with_nothing_written_is_durable_through_every_epoch_below_it() {
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    assert_eq!(harness.seal(3), DurabilityRequest::Submitted);
    expect_durable(&mut harness, 3);
    for id in [1, 2, 3] {
        assert_eq!(harness.state(id), EpochState::Durable);
    }
    harness.finish();
}

#[test]
fn a_seal_made_while_its_writes_are_in_flight_is_reported_after_them() {
    const WRITES: u32 = 16;
    const SIZE: usize = 4096;
    let temp = temp(&vec![0; SIZE * WRITES as usize]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    for i in 0..WRITES {
        harness.write(GIVEN, u64::from(i) * SIZE as u64, &[i as u8; SIZE], 1, i);
    }
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let mut completions = 0;
    loop {
        match harness.next_entry() {
            Entry::Op(_) => completions += 1,
            Entry::Durable { through } => {
                assert_eq!(through, harness.epoch(1));
                break;
            }
            other => panic!("expected a completion or Durable, got {other:?}"),
        }
    }
    assert_eq!(completions, WRITES, "Durable follows every write it covers");
    harness.finish();
}

#[test]
fn successive_seals_are_reported_in_order_each_after_its_own_writes() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    harness.write(GIVEN, 0, b"one", 1, 1);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    harness.write(GIVEN, 8, b"two", 2, 2);
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    let mut durable = Vec::new();
    let mut completions = 0;
    while durable.len() < 2 {
        match harness.next_entry() {
            Entry::Op(_) => completions += 1,
            Entry::Durable { through } => durable.push(through),
            other => panic!("expected a completion or Durable, got {other:?}"),
        }
    }
    assert_eq!(durable, [harness.epoch(1), harness.epoch(2)]);
    assert_eq!(completions, 2);
    harness.finish();
}

#[test]
fn writes_at_or_below_the_seal_are_refused_with_what_they_took() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], vec![vec![0; 16]]));
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    expect_durable(&mut harness, 2);

    for (id, context) in [(2, 1), (1, 2)] {
        let epoch = harness.epoch(id);
        let error = harness
            .ring
            .write(GIVEN, 0, vec![7], harness.default.at(id), context)
            .expect_err("a write at or below the seal");
        assert!(
            matches!(error.reason, PushRefusal::Sealed { epoch: e, sealed_through: 2 } if e == epoch),
            "{:?}",
            error.reason
        );
        assert_eq!((error.buffer, error.context), (Some(vec![7]), context));
    }
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let error = harness
        .ring
        .write_registered(GIVEN, 0, span, harness.default.at(2), 3)
        .expect_err("a registered write at the seal");
    assert!(matches!(
        error.reason,
        PushRefusal::Sealed {
            sealed_through: 2,
            ..
        }
    ));
    assert_eq!((error.buffer, error.context), (None, 3));
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "a refused write completes nothing"
    );

    harness.write(GIVEN, 0, b"later", 3, 4);
    harness.read(GIVEN, 0, 4, 5);
    harness.next_n(2);
    harness.finish();
}

#[test]
fn asking_again_at_or_below_a_durable_seal_reports_durable() {
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    expect_durable(&mut harness, 2);
    for id in [2, 1] {
        assert_eq!(
            harness.seal(id),
            DurabilityRequest::AlreadySealed(EpochState::Durable)
        );
    }
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "asking again is a no-op"
    );
    harness.finish();
}

/// A file routed only to the consumer's provider leaves its seal pending, because providers are
/// not asked anything before DI-3.2.6. `Serves` panics if asked, so this also shows it is not.
#[test]
fn a_pending_seal_reports_pending_and_holds_back_a_later_one() {
    let temp = temp(&[0; 16]);
    let ring = Ring::new(setup(
        vec![file(GIVEN.0, &temp, &["log"])],
        Vec::new(),
        serves(&["log"]),
    ))
    .expect("build an instance with a provider")
    .0;
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();

    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    assert_eq!(
        harness.seal(1),
        DurabilityRequest::AlreadySealed(EpochState::Pending)
    );
    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "no Durable passes a pending seal"
    );
    assert_eq!(harness.state(2), EpochState::Pending);
    assert_eq!(harness.state(3), EpochState::Open);
    assert_eq!(harness.durable(), None);
    assert_eq!(harness.sealed(), Some(2));
    harness.finish();
}

#[test]
fn a_failed_write_is_not_covered_and_does_not_hold_its_seal() {
    let good = temp(&[0; 16]);
    let refused = temp(&[0; 16]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &good)], Vec::new()));
    harness
        .ring
        .add_file(ADDED, read_only(&refused))
        .expect("add a read-only file");
    let failing = harness.write(ADDED, 0, b"refused", 1, 1);
    harness.write(GIVEN, 0, b"kept", 1, 2);
    for completion in harness.next_n(2) {
        assert_eq!(
            matches!(completion.outcome, Outcome::Failed(_)),
            completion.id == failing,
            "{:?}",
            completion.outcome
        );
    }
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    expect_durable(&mut harness, 1);
    harness.finish();
}

#[test]
fn another_instances_lineage_is_refused_and_has_no_seal() {
    let mut ring = instance(Vec::new(), Vec::new());
    let handle = empty().default_lineage();
    let foreign = handle.lineage();
    let error = ring
        .make_durable_through(handle.at(1))
        .expect_err("a foreign lineage");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(ring.sealed_through(foreign), Err(UnknownLineage(foreign)));
    assert_eq!(ring.durable_through(foreign), Err(UnknownLineage(foreign)));
    let own = Epoch::new(ring.default_lineage().lineage(), 1);
    assert_eq!(
        ring.epoch_state(own),
        Ok(EpochState::Open),
        "nothing was sealed"
    );
    assert!(ring.pop().expect("pop").is_none());
}

#[test]
fn the_state_of_another_instances_epoch_is_refused() {
    let ring = instance(Vec::new(), Vec::new());
    let foreign = empty().default_lineage().lineage();
    assert_eq!(
        ring.epoch_state(Epoch::new(foreign, 1)),
        Err(UnknownLineage(foreign))
    );
}
