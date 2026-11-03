// Copyright (c) 2026 Mike Grier
//! Abandoned epochs above the mark (DI-D-35, DI-D-109): kept, each pinning the place of one of its
//! writes, until the mark passes them or their lineage ends or is retired, and never more than the
//! operations declared, whatever other failure still holds the same writes.

use super::{A, B};
use super::{DEFAULT, Lineage, Ops, failed, lineage, submitted, tokens};
use crate::durability::{DEFAULT_LINEAGE as D, Gate, GateState, Key, Reach, State};
use crate::ids::{FailureId, FailureToken};
use crate::test_support::CAPACITY;
use crate::types::{FailureCause, ImportScope, Resolution};

/// A failure the consumer learned of, reaching every uncovered write: its token.
fn import(lineage: &mut Lineage) -> FailureToken {
    let (_, due) = lineage.import(
        FailureCause::Imported {
            scope: ImportScope::All,
        },
        &Reach::All,
    );
    tokens(due).remove(0)
}

/// Abandon the failure `token` resolves, and pop its `Abandoned`, as the I/O core reports it.
fn abandon(lineage: &mut Lineage, token: FailureToken) {
    let id: FailureId = token.id();
    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    lineage.0.forget(id);
}

fn at(lineage: Key, epoch: u64) -> Gate<u64> {
    Gate { lineage, epoch }
}

/// Every place of the window held by an abandoned epoch, each of a different write in a different
/// epoch, with a second failure still suspecting every one of those writes: the table reaches the
/// operations declared, and the places stay one each.
#[test]
fn the_abandoned_epochs_reach_the_operations_and_no_further() {
    let operations = CAPACITY.operations;
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let writes: Vec<_> = (1..=operations as u64)
        .map(|epoch| ops.push(&mut lineage, epoch, A, DEFAULT))
        .collect();
    let first = import(&mut lineage);
    let second = import(&mut lineage);
    for write in &writes {
        lineage.completed(*write, failed());
    }
    assert_eq!(
        lineage.window_places(),
        operations,
        "each failed write kept for both failures"
    );

    abandon(&mut lineage, first);
    assert_eq!(lineage.abandoned_kept(), operations, "every epoch, once");
    assert_eq!(
        lineage.window_places(),
        operations,
        "an abandoned epoch takes a place its write already held: the other failure's"
    );
    for epoch in 1..=operations as u64 {
        assert_eq!(lineage.state(epoch), State::Abandoned);
    }

    abandon(&mut lineage, second);
    assert_eq!(
        lineage.abandoned_kept(),
        operations,
        "abandoning the same epochs again keeps nothing more"
    );
    assert_eq!(
        lineage.window_places(),
        operations,
        "both failures popped; the abandoned epochs still hold the places"
    );

    submitted(lineage.seal(operations as u64));
    assert_eq!(lineage.durable_through(), Some(operations as u64));
    assert_eq!(lineage.abandoned_kept(), 0, "the mark passed every one");
    assert_eq!(lineage.window_places(), 0);
    assert_eq!(lineage.state(1), State::Durable, "as the mark means it");
}

/// The mark forgets only what it passed: an abandoned epoch above it stays, and a gate on it is
/// still abandoned, while one at or below it is answered by the mark.
#[test]
fn the_mark_forgets_the_abandoned_epochs_it_passes_and_no_others() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let low = ops.push(&mut lineage, 2, A, DEFAULT);
    let high = ops.push(&mut lineage, 5, B, DEFAULT);
    let token = import(&mut lineage);
    lineage.completed(low, failed());
    lineage.completed(high, failed());
    abandon(&mut lineage, token);
    assert_eq!(lineage.abandoned_kept(), 2);
    assert_eq!(lineage.gate_state(at(D, 2)), GateState::Abandoned);

    submitted(lineage.seal(3));
    assert_eq!(lineage.durable_through(), Some(3));
    assert_eq!(lineage.abandoned_kept(), 1, "5 is above the mark");
    assert!(!lineage.is_abandoned(2) && lineage.is_abandoned(5));
    assert_eq!(lineage.window_places(), 1, "5's write keeps its place");
    assert_eq!(
        lineage.gate_state(at(D, 2)),
        GateState::Durable,
        "at or below the mark, the mark answers: durable or abandoned"
    );
    assert_eq!(lineage.gate_state(at(D, 5)), GateState::Abandoned);
}

/// Ending a lineage forgets its abandoned epochs: it answers for nothing more.
#[test]
fn ending_a_lineage_forgets_its_abandoned_epochs() {
    let mut lineage = lineage();
    let minted = lineage.mint(None);
    let mut ops = Ops::new();
    let write = ops.push_to(&mut lineage, minted, 2, A, DEFAULT, &[]);
    let token = import(&mut lineage);
    lineage.completed(write, failed());
    abandon(&mut lineage, token);
    assert_eq!(lineage.abandoned_kept(), 1);
    assert_eq!(lineage.window_places(), 1);
    lineage.end(minted);
    assert_eq!(lineage.abandoned_kept(), 0);
    assert_eq!(lineage.window_places(), 0);
    assert!(!lineage.0.is_abandoned(minted, 2), "not live");
}

/// Retiring a live lineage forgets its abandoned epochs too: an abandoned epoch holds nothing a
/// retire is refused for, since its writes are covered or failed.
#[test]
fn retiring_a_lineage_forgets_its_abandoned_epochs() {
    let mut lineage = lineage();
    let minted = lineage.mint(None);
    let mut ops = Ops::new();
    let write = ops.push_to(&mut lineage, minted, 2, A, DEFAULT, &[]);
    let token = import(&mut lineage);
    lineage.completed(write, failed());
    abandon(&mut lineage, token);
    assert_eq!(lineage.abandoned_kept(), 1);
    lineage.retire(minted).expect("nothing holds it");
    assert_eq!(lineage.abandoned_kept(), 0);
    assert_eq!(lineage.window_places(), 0);
}
