// Copyright (c) 2026 Mike Grier
//! Lineage handles (DI-3.2.5.1), against real files: minting one, ending it by releasing its last
//! handle or by `end_lineage`, retiring it, and a failure spanning two lineages healing in each.
//! Every entry is reported to the conformance oracle, so each test also checks that an ended
//! lineage answers nothing more.

use super::empty;
use super::pushes::{ADDED, GIVEN, Harness, given, instance, temp};
use crate::contract::DurableRing;
use crate::ids::{FailureToken, LineageHandle};
use crate::types::{
    DurabilityRequest, EndLineageRefusal, Entry, Epoch, EpochState, ImportScope, Resolution,
    RetireLineageRefusal, UnknownLineage,
};

/// `ERROR_IO_DEVICE`, the failure the seam is armed with.
const IO_DEVICE: u32 = 1117;

/// An instance with `GIVEN` and `ADDED`, and a lineage minted beside its default.
fn two(given_temp: &super::TempFile, added_temp: &super::TempFile) -> (Harness, LineageHandle) {
    let mut harness = Harness::new(instance(
        vec![given(GIVEN, given_temp), given(ADDED, added_temp)],
        Vec::new(),
    ));
    let minted = harness.ring.mint_lineage(Some("log".to_owned()));
    (harness, minted)
}

/// The next entry, which must be `LineageEnded` for `handle`'s lineage.
fn expect_ended(harness: &mut Harness, lineage: crate::ids::Lineage) -> Option<u64> {
    match harness.next_entry() {
        Entry::LineageEnded {
            lineage: ended,
            abandoned_through,
        } => {
            assert_eq!(ended, lineage);
            abandoned_through
        }
        other => panic!("expected LineageEnded, got {other:?}"),
    }
}

/// The next entry, which must be `Durable` through `through`.
fn expect_durable(harness: &mut Harness, through: Epoch<super::V>) {
    match harness.next_entry() {
        Entry::Durable { through: reported } => assert_eq!(reported, through),
        other => panic!("expected Durable through {through:?}, got {other:?}"),
    }
}

#[test]
fn a_minted_lineage_is_listed_and_made_durable_on_its_own() {
    let (a, b) = (temp(&[0; 64]), temp(&[0; 64]));
    let (mut harness, minted) = two(&a, &b);
    let lineage = minted.lineage();
    let listed = harness.ring.lineages();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].lineage, lineage);
    assert_eq!(listed[1].description.as_deref(), Some("log"));
    assert!(!listed[1].is_default);
    assert_ne!(lineage, harness.lineage());

    harness.write_in(&minted, GIVEN, 0, b"minted", 1, 1);
    harness.next();
    assert_eq!(harness.seal_in(&minted, 1), DurabilityRequest::Submitted);
    expect_durable(&mut harness, Epoch::new(lineage, 1));
    assert_eq!(harness.ring.durable_through(lineage), Ok(Some(1)));
    assert_eq!(harness.durable(), None, "the default sealed nothing");
    harness.finish();
}

#[test]
fn releasing_the_last_handle_ends_the_lineage_and_a_copy_keeps_it() {
    let (a, b) = (temp(&[0; 64]), temp(&[0; 64]));
    let (mut harness, minted) = two(&a, &b);
    let lineage = minted.lineage();
    harness.write_in(&minted, GIVEN, 0, b"pending", 3, 1);
    harness.next();

    let copy = minted.clone();
    drop(minted);
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "a copy is still held"
    );
    assert_eq!(harness.ring.lineages().len(), 2);

    drop(copy);
    assert_eq!(expect_ended(&mut harness, lineage), Some(3));
    assert_eq!(harness.ring.lineages().len(), 1);
    assert_eq!(
        harness.ring.durable_through(lineage),
        Err(UnknownLineage(lineage))
    );
    assert_eq!(
        harness.ring.epoch_state(Epoch::new(lineage, 3)),
        Err(UnknownLineage(lineage))
    );
    assert_eq!(
        harness.ring.import_failure(ImportScope::Lineage(lineage)),
        Err(UnknownLineage(lineage))
    );
    harness.finish();
}

#[test]
fn end_lineage_ends_through_the_last_handle_and_hands_back_any_other() {
    let (a, b) = (temp(&[0; 64]), temp(&[0; 64]));
    let (mut harness, minted) = two(&a, &b);
    let lineage = minted.lineage();

    let error = harness
        .ring
        .end_lineage(harness.default.clone())
        .expect_err("the default");
    assert!(matches!(error.reason, EndLineageRefusal::Default));
    assert_eq!(error.handle.lineage(), harness.lineage());

    let copy = minted.clone();
    let error = harness.ring.end_lineage(copy).expect_err("shared");
    assert!(matches!(error.reason, EndLineageRefusal::Shared));
    drop(error);
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "the refused copy, dropped, was not the last"
    );

    let foreign = empty().mint_lineage(None);
    let error = harness.ring.end_lineage(foreign).expect_err("foreign");
    assert!(matches!(error.reason, EndLineageRefusal::Foreign(_)));

    harness.ring.end_lineage(minted).expect("the last handle");
    assert_eq!(expect_ended(&mut harness, lineage), None);
    assert!(harness.ring.pop().expect("pop").is_none(), "ended once");
    harness.finish();
}

#[test]
fn retire_lineage_is_refused_while_anything_holds_it_and_abandons_nothing() {
    let (a, b) = (temp(&[0; 64]), temp(&[0; 64]));
    let (mut harness, minted) = two(&a, &b);
    let lineage = minted.lineage();
    harness.write_in(&minted, GIVEN, 0, b"uncovered", 1, 1);
    harness.next();

    let error = harness.ring.retire_lineage(minted).expect_err("uncovered");
    let busy = match error.reason {
        RetireLineageRefusal::Busy(busy) => busy,
        other => panic!("expected Busy, got {other:?}"),
    };
    assert_eq!(busy.lineage, lineage);
    assert_eq!(
        (busy.in_flight, busy.uncovered, busy.failures.len()),
        (0, 1, 0)
    );
    let minted = error.handle;

    let copy = minted.clone();
    let error = harness.ring.retire_lineage(copy).expect_err("shared");
    assert!(matches!(error.reason, RetireLineageRefusal::Shared));
    drop(error);
    let error = harness
        .ring
        .retire_lineage(harness.default.clone())
        .expect_err("the default");
    assert!(matches!(error.reason, RetireLineageRefusal::Default));
    let foreign = empty().mint_lineage(None);
    let error = harness.ring.retire_lineage(foreign).expect_err("foreign");
    assert!(matches!(error.reason, RetireLineageRefusal::Foreign(_)));

    harness.seal_in(&minted, 1);
    expect_durable(&mut harness, Epoch::new(lineage, 1));
    harness
        .ring
        .retire_lineage(minted)
        .expect("nothing holds it");
    assert!(
        harness.ring.pop().expect("pop").is_none(),
        "retiring abandons nothing, so reports nothing"
    );
    assert_eq!(
        harness.ring.sealed_through(lineage),
        Err(UnknownLineage(lineage))
    );
    assert_eq!(harness.ring.lineages().len(), 1);
    harness.finish();
}

#[test]
fn an_import_scoped_to_a_lineage_suspects_only_its_writes() {
    let (a, b) = (temp(&[0; 64]), temp(&[0; 64]));
    let (mut harness, minted) = two(&a, &b);
    let own = harness.write(GIVEN, 0, b"own", 1, 1);
    let theirs = harness.write_in(&minted, ADDED, 0, b"theirs", 1, 2);
    harness.next_n(2);
    for (scope, expected) in [
        (ImportScope::Lineage(minted.lineage()), vec![theirs]),
        (ImportScope::Lineage(harness.lineage()), vec![own]),
        (ImportScope::All, vec![own, theirs]),
    ] {
        harness.ring.import_failure(scope.clone()).expect("import");
        match harness.next_entry() {
            Entry::Failed(failed) => {
                let suspects: Vec<_> = failed.suspect.writes().iter().map(|w| w.op).collect();
                assert_eq!(suspects, expected, "{scope:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
    harness.finish();
}

#[test]
fn a_handle_outliving_its_instance_releases_harmlessly() {
    let ring = instance(Vec::new(), Vec::new());
    let minted = {
        let mut ring = ring;
        let minted = ring.mint_lineage(None);
        drop(ring);
        minted
    };
    let lineage = minted.lineage();
    drop(minted);
    let other = empty();
    assert_eq!(
        other.durable_through(lineage),
        Err(UnknownLineage(lineage)),
        "and names nothing anywhere else"
    );
}

/// A failure suspecting a held write of each lineage holds both, and a heal takes effect in each
/// at its own next seal (DI-D-41): the default's `Durable` comes before the failure's `Healed`,
/// which waits for the minted lineage's seal.
#[test]
fn a_failure_spanning_two_lineages_heals_in_each_at_its_own_seal() {
    let (a, b) = (temp(&[0; 64]), temp(&[0; 64]));
    let (mut harness, minted) = two(&a, &b);
    let lineage = minted.lineage();
    let own = harness.write(GIVEN, 0, b"own", 1, 1);
    let theirs = harness.write_in(&minted, ADDED, 0, b"theirs", 1, 2);
    harness.next_n(2);

    harness.ring.fail_next_flush(GIVEN, IO_DEVICE);
    harness.seal(1);
    let failed = match harness.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    };
    let suspects: Vec<_> = failed.suspect.writes().iter().map(|w| w.op).collect();
    assert_eq!(suspects, [own, theirs], "a held write of each lineage");
    heal(&mut harness, failed.token);

    assert_eq!(harness.seal(2), DurabilityRequest::Submitted);
    let (one, two) = (harness.epoch(1), harness.epoch(2));
    expect_durable(&mut harness, one);
    expect_durable(&mut harness, two);
    assert_eq!(harness.ring.failures().len(), 1, "it still holds the other");
    assert_eq!(
        harness.ring.epoch_state(Epoch::new(lineage, 1)),
        Ok(EpochState::Open)
    );

    assert_eq!(harness.seal_in(&minted, 1), DurabilityRequest::Submitted);
    loop {
        match harness.next_entry() {
            Entry::Marked { .. } => continue,
            Entry::Healed { failure, .. } => {
                assert_eq!(failure, failed.id);
                break;
            }
            other => panic!("expected Healed, got {other:?}"),
        }
    }
    expect_durable(&mut harness, Epoch::new(lineage, 1));
    assert!(harness.ring.failures().is_empty());
    harness.finish();
}

/// Ending a lineage a heal still waits for lets the heal take effect.
#[test]
fn ending_a_lineage_a_heal_waits_for_reports_the_heal() {
    let (a, b) = (temp(&[0; 64]), temp(&[0; 64]));
    let (mut harness, minted) = two(&a, &b);
    let lineage = minted.lineage();
    harness.write(GIVEN, 0, b"own", 1, 1);
    harness.write_in(&minted, ADDED, 0, b"theirs", 1, 2);
    harness.next_n(2);
    let id = harness
        .ring
        .import_failure(ImportScope::All)
        .expect("import");
    let token = match harness.next_entry() {
        Entry::Failed(failed) => failed.token,
        other => panic!("expected Failed, got {other:?}"),
    };
    heal(&mut harness, token);
    harness.seal(1);
    assert!(
        matches!(harness.next_entry(), Entry::Marked { failure, .. } if failure == id),
        "the seal's flush covered the default's suspect write"
    );
    let one = harness.epoch(1);
    expect_durable(&mut harness, one);

    drop(minted);
    assert_eq!(expect_ended(&mut harness, lineage), Some(1));
    match harness.next_entry() {
        Entry::Healed { failure, .. } => assert_eq!(failure, id),
        other => panic!("expected Healed, got {other:?}"),
    }
    harness.finish();
}

fn heal(harness: &mut Harness, token: FailureToken) {
    let id = token.id();
    harness
        .ring
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    harness.oracle.healed(id);
}
