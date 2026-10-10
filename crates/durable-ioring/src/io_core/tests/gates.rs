// Copyright (c) 2026 Mike Grier
//! Gates (DI-D-23, DI-D-43): holding an operation until an epoch is durable, releasing it, and
//! ending it as `NeverIssued`.

use super::{KEY, Rig, V, shown};
use crate::backend::{IoBackend, RefusalCause};
use crate::ids::OpId;
use crate::types::{
    Entry, Epoch, ImportScope, OpKind, Outcome, PushRefusal, ReadOptions, RemoveTargetError,
    Resolution, WriteOptions,
};

fn gated(gate: Epoch<V>) -> WriteOptions<V> {
    WriteOptions::new().gate(gate)
}

/// The token of the only failure the queue reports, after draining it.
fn only_failure(rig: &mut Rig) -> crate::ids::FailureToken {
    let mut tokens = rig.entries().into_iter().filter_map(|entry| match entry {
        Entry::Failed(failed) => Some(failed.token),
        _ => None,
    });
    let token = tokens.next().expect("a failure");
    assert!(tokens.next().is_none(), "only one failure");
    token
}

#[test]
fn a_write_gated_on_an_epoch_not_yet_durable_is_held_then_released() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"first").expect("accepted");
    let gate = rig.epoch(1);
    let held = rig
        .write_with(KEY, rig.epoch(2), 5, b"second", gated(gate))
        .expect("held");
    assert_eq!(
        rig.backend.queued(),
        1,
        "only the first write reached the backend"
    );
    rig.seal(1);
    rig.run();
    assert_eq!(
        shown(&mut rig),
        ["op0 write@1 5 write", "durable 1", "op1 write@2 6 write"]
    );
    assert_eq!(held.seq, 1, "a held operation spends its id when held");
    assert_eq!(rig.target().contents(), b"firstsecond");
}

#[test]
fn a_gate_already_durable_holds_nothing() {
    let mut rig = Rig::new();
    rig.seal(1);
    rig.run();
    rig.entries();
    let gate = rig.epoch(1);
    rig.write_with(KEY, rig.epoch(2), 0, b"x", gated(gate))
        .expect("pushed at once");
    assert_eq!(rig.backend.queued(), 1);
}

#[test]
fn a_gated_read_is_held_then_released() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"abc").expect("accepted");
    let gate = rig.epoch(1);
    rig.read_with(0, 3, ReadOptions::new().gate(gate))
        .expect("held");
    rig.backend.submit().expect("submit");
    let write = rig.backend.pop().expect("never fails").expect("the write");
    let actions = rig.core.complete(write);
    assert!(actions.submit.is_empty(), "the gate is not yet sealed");
    rig.act(actions);
    rig.seal(1);
    rig.run();
    let entries = rig.entries();
    let Some(Entry::Op(read)) = entries.last() else {
        panic!("the read last: {entries:?}");
    };
    assert_eq!(read.kind, OpKind::Read);
    assert_eq!(read.buffer.as_deref(), Some(b"abc".as_slice()));
}

#[test]
fn a_gate_in_another_core_is_refused() {
    let mut rig = Rig::new();
    let other = super::Core::new(
        super::Ticks::default(),
        std::sync::Arc::new(crate::NoDiagnostics),
    );
    let foreign = Epoch::new(other.default_lineage(), 1);
    let refused = rig
        .write_with(KEY, rig.epoch(1), 0, b"x", gated(foreign))
        .expect_err("foreign gate");
    assert!(matches!(refused.reason, PushRefusal::UnknownLineage(_)));
    assert_eq!(refused.buffer.as_deref(), Some(b"x".as_slice()));
}

#[test]
fn a_gate_on_an_ended_lineage_is_refused() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    let actions = rig_end(&mut rig, lineage);
    rig.act(actions);
    let refused = rig
        .write_with(KEY, rig.epoch(1), 0, b"x", gated(Epoch::new(lineage, 1)))
        .expect_err("ended");
    assert!(matches!(refused.reason, PushRefusal::UnknownLineage(_)));
}

fn rig_end(rig: &mut Rig, lineage: crate::ids::Lineage) -> super::CoreActions<V, Vec<u8>> {
    rig.core
        .end_lineage(lineage)
        .expect("live, not the default")
}

#[test]
fn a_gate_on_an_abandoned_epoch_is_refused() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    let (_, actions) = rig
        .core
        .import_failure(ImportScope::All)
        .expect("all is always in scope");
    rig.act(actions);
    let token = only_failure(&mut rig);
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("our token");
    rig.act(actions);
    let gate = rig.epoch(1);
    let refused = rig
        .write_with(KEY, rig.epoch(2), 0, b"x", gated(gate))
        .expect_err("abandoned gate");
    assert!(matches!(refused.reason, PushRefusal::GateAbandoned { .. }));
}

#[test]
fn a_held_operation_whose_gate_is_abandoned_never_issues() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.entries();
    let gate = rig.epoch(1);
    let held = rig
        .write_with(KEY, rig.epoch(2), 0, b"held", gated(gate))
        .expect("held");
    let (_, actions) = rig.core.import_failure(ImportScope::All).expect("in scope");
    rig.act(actions);
    let token = only_failure(&mut rig);
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("our token");
    assert!(actions.deliver);
    rig.act(actions);
    let entries = rig.entries();
    let op = entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Op(op) if op.id == held => Some(op),
            _ => None,
        })
        .expect("the held write's completion");
    let Outcome::NeverIssued { abandoned } = op.outcome else {
        panic!("never issued: {:?}", op.outcome);
    };
    assert_eq!(abandoned, gate);
    assert_eq!(op.buffer.as_deref(), Some(b"held".as_slice()));
    assert_eq!(op.context, "write");
    assert_eq!(rig.backend.queued(), 0, "it never reached the backend");
}

#[test]
fn a_held_write_whose_own_lineage_ends_never_issues() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    rig.write(1, 0, b"x").expect("accepted");
    let gate = rig.epoch(1);
    rig.write_with(KEY, Epoch::new(lineage, 1), 0, b"mine", gated(gate))
        .expect("held");
    let actions = rig_end(&mut rig, lineage);
    rig.act(actions);
    let shown = shown(&mut rig);
    assert!(
        shown
            .iter()
            .any(|line| line == "op1 write@1 never issued write"),
        "{shown:?}"
    );
}

#[test]
fn a_write_closing_a_gate_cycle_is_refused() {
    let mut rig = Rig::new();
    let a = rig.core.mint_lineage(None);
    let b = rig.core.mint_lineage(None);
    rig.write_with(KEY, Epoch::new(a, 1), 0, b"a", gated(Epoch::new(b, 5)))
        .expect("held");
    let refused = rig
        .write_with(KEY, Epoch::new(b, 1), 0, b"b", gated(Epoch::new(a, 1)))
        .expect_err("a cycle");
    let PushRefusal::GateCycle { cycle } = refused.reason else {
        panic!("a cycle: {:?}", refused.reason);
    };
    assert_eq!(cycle.first(), Some(&Epoch::new(b, 1)));
    assert_eq!(refused.buffer.as_deref(), Some(b"b".as_slice()));
}

#[test]
fn a_target_is_busy_while_an_operation_waits_for_a_gate() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    for id in [4, 2] {
        rig.read_with(0, 1, ReadOptions::new().gate(Epoch::new(lineage, id)))
            .expect("held");
    }
    let Err(RemoveTargetError::Busy(busy)) = rig.core.remove_target(KEY) else {
        panic!("busy");
    };
    assert_eq!(busy.held_for_gate, 2);
    assert_eq!(busy.lowest_gates, [Epoch::new(lineage, 2)]);
}

#[test]
fn close_hands_back_held_operations_in_push_order() {
    let mut rig = Rig::new();
    let lineage = rig.core.mint_lineage(None);
    let gate = Epoch::new(lineage, 1);
    let first = rig
        .write_with(KEY, rig.epoch(1), 0, b"one", gated(gate))
        .expect("held");
    let second = rig
        .read_with(0, 2, ReadOptions::new().gate(gate))
        .expect("held");
    let leftovers = rig.core.close();
    let ids: Vec<OpId> = leftovers.held.iter().map(|held| held.id).collect();
    assert_eq!(ids, [first, second]);
    assert_eq!(leftovers.held[0].gate, gate);
    assert_eq!(leftovers.held[0].buffer.as_deref(), Some(b"one".as_slice()));
    assert_eq!(leftovers.held[1].kind, OpKind::Read);
    assert_eq!(leftovers.lineages.len(), 2);
}

#[test]
fn a_released_operation_the_backend_refuses_is_kept_and_emitted_again() {
    let mut rig = Rig::with_queues(1, 16);
    rig.seal(1);
    rig.entries();
    let lineage = rig.core.mint_lineage(None);
    rig.write_with(KEY, Epoch::new(lineage, 1), 0, b"g", WriteOptions::new())
        .expect("accepted");
    rig.backend.submit().expect("submit");
    let gate_write = rig.backend.pop().expect("never fails").expect("the write");
    let actions = rig.core.complete(gate_write);
    rig.act(actions);
    rig.read_with(0, 1, ReadOptions::new().gate(Epoch::new(lineage, 1)))
        .expect("held");
    rig.seal_in(lineage, 1);
    // The commit fills the one-entry submission queue; the release that follows it is refused.
    rig.backend.submit().expect("submit");
    let commit = rig.backend.pop().expect("never fails").expect("the commit");
    rig.read(0, 1).expect("fills the queue");
    let actions = rig.core.complete(commit);
    assert_eq!(actions.submit.len(), 1, "the released read");
    let refused = rig
        .backend
        .push(actions.submit.into_iter().next().expect("one"))
        .expect_err("full");
    assert_eq!(refused.cause, RefusalCause::SubmitNeeded);
    rig.core.refused(refused);
    rig.backend.submit().expect("submit");
    let again = rig.core.retry();
    assert_eq!(again.submit.len(), 1, "kept");
    rig.act(again);
    rig.run();
    let reads = rig
        .entries()
        .into_iter()
        .filter(|entry| matches!(entry, Entry::Op(op) if op.kind == OpKind::Read))
        .count();
    assert_eq!(reads, 2);
}
