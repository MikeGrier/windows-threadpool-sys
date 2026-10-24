// Copyright (c) 2026 Mike Grier
//! The lists a call collects into and hands the driver (DI-D-97, DI-23.12.2): made when the core
//! is built, at sizes a bound on one call gives each, and reused. A list pushed onto past its room
//! is an inconsistency and a panic, so each test here makes one call put as much in a list as its
//! bound allows -- the operations declared, the lineages, or the flush parts -- and passes only if
//! the list was made that large.

use std::sync::Arc;

use super::pairs::Checked;
use super::{Core, KEY, Rig, Ticks, domain, listed, nz};
use crate::backend::RefusalCause;
use crate::capacity::Capacity;
use crate::diagnostics::{DiagnosticSink, NoDiagnostics};
use crate::memory::{Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When};
use crate::test_support::CAPACITY;
use crate::types::{Epoch, IoTargetKey, PushRefusal, ReadOptions, WriteOptions};

/// The operations every test here declares.
const OPERATIONS: usize = 4;

fn declared(f: impl FnOnce(&mut Capacity)) -> Capacity {
    let mut capacity = CAPACITY;
    capacity.operations = OPERATIONS;
    f(&mut capacity);
    capacity
}

fn core_with(capacity: Capacity) -> Core {
    Core::new(Ticks::default(), Arc::new(NoDiagnostics), capacity).expect("a valid capacity")
}

/// A rig over a core built for `capacity`, with a target under each of the keys 1 to `targets`.
fn rig_with(capacity: Capacity, targets: u64) -> Rig {
    let mut rig = Rig::over(core_with(capacity));
    for key in 1..=targets {
        rig.attach(IoTargetKey(key), MemoryTarget::new());
    }
    rig
}

/// One write in epoch 1 of the default lineage to each target, completed.
fn one_write_to_each(rig: &mut Rig) {
    for key in 1..=OPERATIONS as u64 {
        rig.write_to(IoTargetKey(key), 1, 0, b"x").expect("room");
    }
    rig.run();
}

#[test]
fn a_seal_over_as_many_files_as_operations_makes_every_flush_due_in_one_change() {
    let mut rig = rig_with(declared(|c| c.targets = OPERATIONS), OPERATIONS as u64);
    one_write_to_each(&mut rig);
    let (_, due) = rig.sealed(rig.epoch(1));
    assert_eq!(
        due.submit.len(),
        OPERATIONS,
        "a commit for each file at once"
    );
    rig.act(due);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn one_commit_covers_as_many_writes_as_operations_at_once() {
    let mut rig = rig_with(declared(|_| {}), 1);
    for offset in 0..OPERATIONS as u64 {
        rig.write_to(KEY, 1, offset, b"x").expect("room");
    }
    rig.run();
    rig.seal(1);
    assert_eq!(rig.core.window_places(), OPERATIONS);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        0,
        "the one commit covered them all"
    );
}

#[test]
fn a_gate_made_durable_releases_as_many_operations_as_declared_in_one_change() {
    let mut rig = rig_with(declared(|_| {}), 1);
    for offset in 0..OPERATIONS as u64 {
        rig.read_with(offset, 1, ReadOptions::new().gate(rig.epoch(1)))
            .expect("held");
    }
    assert_eq!(rig.core.window_places(), OPERATIONS);
    let (_, due) = rig.sealed(rig.epoch(1));
    assert_eq!(
        due.submit.len(),
        OPERATIONS,
        "every held read, released at once"
    );
    rig.act(due);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn a_change_walks_as_many_live_lineages_as_declared() {
    let mut rig = rig_with(declared(|c| c.lineages = 4), 1);
    for _ in 0..3 {
        rig.core.mint_lineage(None).expect("room");
    }
    assert_eq!(listed(&rig.core).len(), 4);
    rig.write_to(KEY, 1, 0, b"x").expect("room");
    rig.seal(1);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

/// A core declaring the default lineage and one more, minted, with a held write of the minted
/// lineage's epoch 1 for each operation declared, each gated on the default lineage's epoch 2.
fn gated_on_the_default(rig: &mut Rig) -> crate::ids::Lineage {
    let minted = rig.core.mint_lineage(None).expect("room");
    let gate = WriteOptions::new().gate(rig.epoch(2));
    for offset in 0..OPERATIONS as u64 {
        rig.write_with(KEY, Epoch::new(minted, 1), offset, b"g", gate)
            .expect("held");
    }
    assert_eq!(rig.core.window_places(), OPERATIONS);
    minted
}

#[test]
fn a_cycle_search_visits_a_gate_for_every_operation_and_every_lineage() {
    let mut rig = rig_with(declared(|c| c.lineages = 2), 1);
    let minted = gated_on_the_default(&mut rig);
    // The search starts at the minted lineage's epoch 1, finds every held write there, and goes
    // on to the default lineage through each of their gates: no cycle, so the push is refused for
    // the room it would need instead.
    let refused = rig
        .write_with(
            KEY,
            rig.epoch(3),
            9,
            b"x",
            WriteOptions::new().gate(Epoch::new(minted, 1)),
        )
        .expect_err("full");
    assert!(matches!(refused.reason, PushRefusal::AtCapacity(_)));
}

#[test]
fn a_traced_cycle_takes_a_step_for_every_operation_and_one_more() {
    let mut rig = rig_with(declared(|c| c.lineages = 2), 1);
    let minted = gated_on_the_default(&mut rig);
    let refused = rig
        .write_with(
            KEY,
            rig.epoch(1),
            9,
            b"x",
            WriteOptions::new().gate(Epoch::new(minted, 1)),
        )
        .expect_err("a cycle");
    let PushRefusal::GateCycle { cycle } = refused.reason else {
        panic!("a cycle: {:?}", refused.reason);
    };
    assert_eq!(
        cycle,
        [
            rig.epoch(1),
            Epoch::new(minted, 1),
            Epoch::new(minted, 1),
            rig.epoch(2)
        ]
    );
}

#[test]
fn as_many_commits_as_operations_refused_and_kept_come_back_together() {
    let script = FaultScript::new().on(
        When::any().op(ScriptOp::Commit),
        Fault::Refuse(RefusalCause::SubmitNeeded),
    );
    let mut rig = rig_with(declared(|c| c.targets = OPERATIONS), 0);
    rig.backend = MemoryBackend::new(nz(8), nz(16)).with_script(script);
    for key in 1..=OPERATIONS as u64 {
        rig.attach(IoTargetKey(key), MemoryTarget::new());
    }
    one_write_to_each(&mut rig);
    rig.seal(1);
    assert_eq!(
        rig.retried().submit.len(),
        OPERATIONS,
        "every commit refused, and kept"
    );
}

#[test]
fn as_many_seals_as_operations_start_together_and_each_asks_the_provider() {
    let core = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        declared(|_| {}),
        vec![domain("log")],
    )
    .expect("a valid capacity");
    let mut rig = Rig::over(core);
    rig.backend = MemoryBackend::new(nz(8), nz(16))
        .with_script(FaultScript::new().on(When::any().op(ScriptOp::Write).nth(1), Fault::Hold));
    rig.attach(KEY, MemoryTarget::new().with_domains([domain("log")]));
    for epoch in 1..=OPERATIONS as u64 {
        rig.write_to(KEY, epoch, epoch, b"x").expect("room");
    }
    rig.run();
    for epoch in 1..=OPERATIONS as u64 {
        rig.seal(epoch);
    }
    assert!(
        rig.requests.is_empty(),
        "every seal waits for the first write"
    );
    rig.backend.release_all();
    rig.run();
    assert_eq!(
        rig.requests.len(),
        OPERATIONS,
        "the first write's completion started every seal at once"
    );
}

#[test]
fn dropping_a_core_ends_as_many_flush_pairs_as_the_flush_parts_declared() {
    // As the flush-pair table's own test: each seal opens a pair for its file's flush and one for
    // each of two served domains, and every one is still open when the core is dropped.
    let sink = Checked::pairs();
    let shown: Arc<dyn DiagnosticSink> = Arc::clone(&sink) as Arc<dyn DiagnosticSink>;
    let core = Core::with_provider(
        Ticks::default(),
        shown,
        declared(|c| c.lineages = 8),
        vec![domain("a"), domain("b")],
    )
    .expect("a valid capacity");
    let mut rig = Rig::over(core);
    rig.backend = MemoryBackend::new(nz(8), nz(16))
        .with_script(FaultScript::new().on(When::any().op(ScriptOp::Commit), Fault::Hold));
    rig.attach(
        KEY,
        MemoryTarget::new().with_domains([domain("a"), domain("b"), domain("z")]),
    );
    for _ in 0..OPERATIONS {
        let minted = rig.core.mint_lineage(None).expect("room");
        rig.write_with(KEY, Epoch::new(minted, 1), 0, b"x", Default::default())
            .expect("room");
        rig.run();
        rig.seal_in(minted, 1);
        rig.run();
    }
    assert_eq!(rig.requests.len(), OPERATIONS, "a request for each seal");
    drop(rig);
    sink.finished();
}
