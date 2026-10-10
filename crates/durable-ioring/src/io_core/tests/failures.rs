// Copyright (c) 2026 Mike Grier
//! Failures (DI-D-12, DI-D-34): importing, resolving, and the inventory.

use std::sync::Arc;

use super::{Core, KEY, Rig, Ticks, shown};
use crate::diagnostics::NoDiagnostics;
use crate::ids::FailureToken;
use crate::memory::MemoryTarget;
use crate::types::{
    Entry, Epoch, EpochState, FailureCause, FlushDomain, ImportScope, PushRefusal, Resolution,
    ResolveRefusal,
};

/// Drain the queue, returning the failures' tokens in order.
fn tokens(rig: &mut Rig) -> Vec<FailureToken> {
    rig.entries()
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .collect()
}

/// One write at `epoch`, completed and not sealed.
fn uncovered(rig: &mut Rig, epoch: u64) {
    rig.write(epoch, 0, b"x").expect("accepted");
    rig.run();
    rig.entries();
}

#[test]
fn an_import_suspects_the_uncovered_writes_in_scope() {
    let mut rig = Rig::new();
    uncovered(&mut rig, 1);
    let (imported, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    assert!(imported.unmatched.is_empty());
    assert!(actions.deliver);
    rig.act(actions);
    let entries = rig.entries();
    let [Entry::Failed(failed)] = entries.as_slice() else {
        panic!("one failure: {entries:?}");
    };
    assert_eq!(failed.id, imported.failure);
    assert!(matches!(failed.cause, FailureCause::Imported { .. }));
    assert_eq!(failed.suspect.writes().len(), 1);
    let listed = rig.core.failures();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, imported.failure);
}

#[test]
fn a_failure_is_stamped_from_the_cores_clock() {
    let mut rig = Rig::new();
    uncovered(&mut rig, 1);
    let first = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(first.1);
    let second = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(second.1);
    let stamps: Vec<u64> = rig.core.failures().iter().map(|f| f.observed).collect();
    assert_eq!(stamps, [0, 1]);
}

#[test]
fn an_import_naming_unknown_domains_reports_them_unmatched() {
    let mut rig = Rig::new();
    rig.attach(
        super::OTHER,
        MemoryTarget::new().with_domains([FlushDomain::new(b"disk".as_slice())]),
    );
    let names = vec![
        FlushDomain::new(b"disk".as_slice()),
        FlushDomain::new(b"typo".as_slice()),
    ];
    let (imported, actions) = rig
        .core
        .import_failure(ImportScope::Domains(names))
        .expect("in scope");
    rig.act(actions);
    assert_eq!(imported.unmatched, [FlushDomain::new(b"typo".as_slice())]);
}

#[test]
fn an_import_confined_to_a_lineage_suspects_only_its_writes() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    uncovered(&mut rig, 1);
    rig.write_with(KEY, Epoch::new(lineage, 1), 0, b"y", Default::default())
        .expect("accepted");
    rig.run();
    rig.entries();
    let (_, actions) = rig
        .core
        .import_failure(ImportScope::Lineage(lineage))
        .expect("live");
    rig.act(actions);
    let entries = rig.entries();
    let [Entry::Failed(failed)] = entries.as_slice() else {
        panic!("one failure: {entries:?}");
    };
    assert_eq!(failed.suspect.writes().len(), 1);
    assert_eq!(failed.suspect.writes()[0].epoch.lineage, lineage);
}

#[test]
fn an_import_naming_a_lineage_not_live_is_refused() {
    let mut rig = Rig::new();
    let other = Core::new(Ticks::default(), Arc::new(NoDiagnostics));
    assert!(
        rig.core
            .import_failure(ImportScope::Lineage(other.default_lineage()))
            .is_err()
    );
    assert!(rig.core.failures().is_empty());
}

#[test]
fn abandoning_a_failure_abandons_its_epochs() {
    let mut rig = Rig::new();
    uncovered(&mut rig, 1);
    let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let token = tokens(&mut rig).pop().expect("a failure");
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("our token");
    rig.act(actions);
    let shown = shown(&mut rig);
    assert!(shown[0].starts_with("Abandoned"), "{shown:?}");
    assert_eq!(
        rig.core.epoch_state(rig.epoch(1)),
        Ok(EpochState::Abandoned)
    );
    let refused = rig.write(1, 0, b"z").expect_err("abandoned");
    assert!(matches!(refused.reason, PushRefusal::EpochAbandoned { .. }));
    assert!(rig.core.failures().is_empty());
}

#[test]
fn a_heal_takes_effect_with_the_next_successful_seal() {
    let mut rig = Rig::new();
    uncovered(&mut rig, 1);
    let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let token = tokens(&mut rig).pop().expect("a failure");
    rig.seal(1);
    rig.run();
    let shown_before = shown(&mut rig);
    assert!(
        shown_before
            .iter()
            .any(|line| line.starts_with("blocked 1")),
        "{shown_before:?}"
    );
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Heal)])
        .expect("our token");
    rig.act(actions);
    rig.write(2, 0, b"again").expect("accepted");
    rig.seal(2);
    rig.run();
    let after = shown(&mut rig);
    assert!(
        after.iter().any(|line| line.starts_with("Healed")),
        "{after:?}"
    );
    assert_eq!(after.last().map(String::as_str), Some("durable 2"));
    assert!(rig.core.failures().is_empty());
}

#[test]
fn a_foreign_token_refuses_the_whole_resolution_and_hands_every_token_back() {
    let mut rig = Rig::new();
    uncovered(&mut rig, 1);
    let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let ours = tokens(&mut rig).pop().expect("a failure");
    let mut other = Rig::new();
    uncovered(&mut other, 1);
    let (_, actions) = other
        .core
        .import_failure(ImportScope::All)
        .expect("in scope");
    other.act(actions);
    let theirs = tokens(&mut other).pop().expect("a failure");
    let refused = rig
        .core
        .resolve(vec![
            (ours, Resolution::Abandon),
            (theirs, Resolution::Abandon),
        ])
        .expect_err("a foreign token");
    assert!(matches!(refused.reason, ResolveRefusal::Foreign(_)));
    assert_eq!(refused.returned.len(), 2);
    assert_eq!(rig.core.failures().len(), 1, "nothing was resolved");
}

#[test]
fn a_dropped_token_can_be_taken_again_and_a_held_one_cannot() {
    let mut rig = Rig::new();
    uncovered(&mut rig, 1);
    let (imported, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let token = tokens(&mut rig).pop().expect("a failure");
    assert!(
        rig.core.take_token(imported.failure).is_none(),
        "still held"
    );
    drop(token);
    let again = rig.core.take_token(imported.failure).expect("dropped");
    let actions = rig
        .core
        .resolve(vec![(again, Resolution::Abandon)])
        .expect("our token");
    rig.act(actions);
    assert!(rig.core.failures().is_empty());
}
