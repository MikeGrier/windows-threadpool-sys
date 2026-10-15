// Copyright (c) 2026 Mike Grier
//! Lineages (DI-D-19, DI-D-30, DI-D-40): minting, enumerating, ending and retiring.

use std::sync::Arc;

use super::{Core, KEY, Rig, Ticks, shown};
use crate::diagnostics::NoDiagnostics;
use crate::types::{EndLineageRefusal, Entry, Epoch, PushRefusal, RetireLineageRefusal, SealError};

#[test]
fn a_minted_lineage_is_listed_after_the_default_with_its_description() {
    let mut rig = Rig::new();
    let minted = rig.core.mint_lineage(Some("log".to_owned()));
    let unnamed = rig.core.mint_lineage(None);
    let listed = rig.core.lineages();
    assert_eq!(listed.len(), 3);
    assert!(listed[0].is_default);
    assert_eq!(listed[0].lineage, rig.core.default_lineage());
    assert_eq!(listed[1].lineage, minted);
    assert_eq!(listed[1].description.as_deref(), Some("log"));
    assert!(!listed[1].is_default);
    assert_eq!(listed[2].lineage, unnamed);
    assert!(listed[2].description.is_none());
}

#[test]
fn lineages_seal_independently() {
    let mut rig = Rig::new();
    let other = rig.core.mint_lineage(None);
    rig.write(1, 0, b"default").expect("accepted");
    rig.write_with(KEY, Epoch::new(other, 9), 7, b"other", Default::default())
        .expect("accepted");
    rig.seal_in(other, 9);
    rig.run();
    assert_eq!(rig.core.durable_through(other), Ok(Some(9)));
    assert_eq!(
        rig.core.durable_through(rig.core.default_lineage()),
        Ok(None)
    );
    assert_eq!(
        rig.core.sealed_through(rig.core.default_lineage()),
        Ok(None)
    );
}

#[test]
fn ending_a_lineage_abandons_what_was_not_durable_and_reports_it() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    rig.write_with(KEY, Epoch::new(lineage, 3), 0, b"x", Default::default())
        .expect("accepted");
    rig.run();
    rig.entries();
    let actions = rig
        .core
        .end_lineage(lineage)
        .expect("live, not the default");
    assert!(actions.deliver);
    rig.act(actions);
    let entries = rig.entries();
    let [
        Entry::LineageEnded {
            lineage: ended,
            abandoned_through,
        },
    ] = entries.as_slice()
    else {
        panic!("one entry: {entries:?}");
    };
    assert_eq!(*ended, lineage);
    assert_eq!(*abandoned_through, Some(3));
}

#[test]
fn an_ended_lineage_is_no_longer_answered_for() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    let actions = rig.core.end_lineage(lineage).expect("live");
    rig.act(actions);
    let refused = rig
        .write_with(KEY, Epoch::new(lineage, 1), 0, b"x", Default::default())
        .expect_err("ended");
    assert!(matches!(refused.reason, PushRefusal::UnknownLineage(_)));
    assert!(matches!(
        rig.core.make_durable_through(Epoch::new(lineage, 1)),
        Err(SealError::UnknownLineage(_))
    ));
    assert!(rig.core.durable_through(lineage).is_err());
    assert!(matches!(
        rig.core.end_lineage(lineage),
        Err(EndLineageRefusal::Foreign(l)) if l == lineage
    ));
    assert!(matches!(
        rig.core.retire_lineage(lineage),
        Err(RetireLineageRefusal::Foreign(l)) if l == lineage
    ));
    assert_eq!(rig.core.lineages().len(), 1);
}

#[test]
fn the_default_lineage_is_neither_ended_nor_retired() {
    let mut rig = Rig::new();
    let default = rig.core.default_lineage();
    assert!(matches!(
        rig.core.end_lineage(default),
        Err(EndLineageRefusal::Default)
    ));
    assert!(matches!(
        rig.core.retire_lineage(default),
        Err(RetireLineageRefusal::Default)
    ));
}

#[test]
fn another_cores_lineage_is_foreign() {
    let mut rig = Rig::new();
    let other = Core::new(Ticks::default(), Arc::new(NoDiagnostics));
    let foreign = other.default_lineage();
    assert!(matches!(
        rig.core.end_lineage(foreign),
        Err(EndLineageRefusal::Foreign(_))
    ));
    assert!(matches!(
        rig.core.retire_lineage(foreign),
        Err(RetireLineageRefusal::Foreign(_))
    ));
}

#[test]
fn a_lineage_with_uncovered_writes_is_busy_until_they_are_covered() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    rig.write_with(KEY, Epoch::new(lineage, 1), 0, b"x", Default::default())
        .expect("accepted");
    let Err(RetireLineageRefusal::Busy(busy)) = rig.core.retire_lineage(lineage) else {
        panic!("busy while in flight");
    };
    assert_eq!(busy.in_flight, 1);
    rig.run();
    let Err(RetireLineageRefusal::Busy(busy)) = rig.core.retire_lineage(lineage) else {
        panic!("busy while uncovered");
    };
    assert_eq!((busy.in_flight, busy.uncovered), (0, 1));
    rig.seal_in(lineage, 1);
    rig.run();
    rig.core.retire_lineage(lineage).expect("nothing holds it");
    assert_eq!(rig.core.lineages().len(), 1);
    assert!(rig.core.durable_through(lineage).is_err());
}

#[test]
fn a_lineage_whose_epoch_gates_an_operation_is_busy() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    rig.write_with(
        KEY,
        rig.epoch(1),
        0,
        b"x",
        crate::types::WriteOptions::new().gate(Epoch::new(lineage, 1)),
    )
    .expect("held");
    let Err(RetireLineageRefusal::Busy(busy)) = rig.core.retire_lineage(lineage) else {
        panic!("busy");
    };
    assert_eq!(busy.held_for_gate, 1);
}

#[test]
fn ending_a_lineage_ends_what_waits_on_it() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    rig.write_with(
        KEY,
        rig.epoch(1),
        0,
        b"x",
        crate::types::WriteOptions::new().gate(Epoch::new(lineage, 1)),
    )
    .expect("held");
    let actions = rig.core.end_lineage(lineage).expect("live");
    rig.act(actions);
    let shown = shown(&mut rig);
    assert!(
        shown
            .iter()
            .any(|line| line == "op0 write@1 never issued write"),
        "{shown:?}"
    );
}
