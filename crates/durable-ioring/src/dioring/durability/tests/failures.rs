// Copyright (c) 2026 Mike Grier
//! Failures, their reach, and their resolution (DI-D-12, DI-D-21, DI-D-34), without a ring.

use std::io;
use std::sync::Arc;

use super::{
    A, B, C, DEFAULT, Lineage, Ops, Seen, State, already, durable, flush, lineage, quiet, seen,
    submitted, tokens,
};
use crate::dioring::DomainId;
use crate::dioring::durability::Reach;
use crate::ids::{FailureId, FailureToken, InstanceId};
use crate::types::{Cause, FileKey, ImportScope, Resolution, ResolveRefusal};

const D: FileKey = FileKey(4);

fn failed() -> io::Result<()> {
    Err(io::Error::from_raw_os_error(1117))
}

fn domains(ids: &[u32]) -> Reach {
    Reach::Domains(
        ids.iter()
            .map(|&d| DomainId(d))
            .collect::<Arc<[DomainId]>>(),
    )
}

fn import(lineage: &mut Lineage, reach: &Reach) -> (FailureId, super::Due<u64, &'static str>) {
    lineage.import(
        Cause::Imported {
            scope: ImportScope::All,
        },
        reach,
    )
}

fn blocked_by(state: State) -> Option<u64> {
    match state {
        State::Blocked(by) => Some(by.seq),
        _ => None,
    }
}

/// A lineage whose write 0, tagged 1 on A, was sealed and whose flush failed, observing failure 0.
fn one_failure(ops: &mut Ops) -> (Lineage, FailureToken) {
    let mut lineage = lineage();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, true);
    submitted(lineage.seal(1));
    let mut found = tokens(lineage.flushed(1, A, failed()));
    assert_eq!(found.len(), 1);
    (lineage, found.remove(0))
}

#[test]
fn a_failed_flush_suspects_every_held_write_in_push_order() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let sealed = ops.push(&mut lineage, 1, A, DEFAULT);
    let failed_write = ops.push(&mut lineage, 1, B, DEFAULT);
    let open = ops.push(&mut lineage, 3, C, DEFAULT);
    lineage.completed(sealed, true);
    lineage.completed(failed_write, false);
    lineage.completed(open, true);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    let in_flight = ops.push(&mut lineage, 2, B, DEFAULT);

    let due = lineage.flushed(1, A, failed());
    assert_eq!(
        seen(&due),
        [Seen::Failed(0, vec![sealed.seq, open.seq, in_flight.seq])],
        "the sealed, the open and the in-flight write, but not the one that failed"
    );
    assert_eq!(blocked_by(lineage.state(1)), Some(0));
    assert_eq!(lineage.durable_through(), None);
    let inventory = lineage.failures();
    assert_eq!(inventory.len(), 1);
    assert!(matches!(inventory[0].cause, Cause::Flush { file: A, .. }));
    assert!(inventory[0].token_live, "the token went out with Failed");
}

#[test]
fn a_suspect_set_is_frozen_and_a_later_seal_it_holds_is_answered_blocked() {
    let mut ops = Ops::new();
    let (mut lineage, token) = one_failure(&mut ops);
    let later = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(later, true);
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, B)]);
    assert_eq!(
        seen(&lineage.flushed(2, B, Ok(()))),
        [Seen::Blocked(2, 0)],
        "2's flush succeeded, but failure 0 holds 1"
    );
    assert_eq!(blocked_by(lineage.state(2)), Some(0));
    let suspects: Vec<u64> = lineage.failures()[0]
        .suspect
        .writes()
        .iter()
        .map(|w| w.op.seq)
        .collect();
    assert_eq!(
        suspects,
        [0],
        "a write pushed after the failure is not added"
    );

    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(
        seen(&due),
        [
            Seen::Abandoned(0, vec![0]),
            Seen::Durable(1),
            Seen::Durable(2)
        ]
    );
    assert_eq!(lineage.state(1), State::Abandoned);
    assert_eq!(lineage.state(2), State::Durable);
}

#[test]
fn a_write_already_covered_is_never_suspected() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let on_a = ops.push(&mut lineage, 1, A, DEFAULT);
    let on_b = ops.push(&mut lineage, 1, B, DEFAULT);
    lineage.completed(on_a, true);
    lineage.completed(on_b, true);
    submitted(lineage.seal(1));
    assert!(quiet(&lineage.flushed(1, A, Ok(()))));
    assert_eq!(
        seen(&lineage.flushed(1, B, failed())),
        [Seen::Failed(0, vec![on_b.seq])],
        "A's flush covered its write before the failure, though the mark has not passed it"
    );

    let mut lineage = super::lineage();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(first, true);
    submitted(lineage.seal(1));
    assert_eq!(durable(&lineage.flushed(1, A, Ok(()))), [1]);
    let second = ops.push(&mut lineage, 2, A, DEFAULT);
    lineage.completed(second, true);
    submitted(lineage.seal(2));
    assert_eq!(
        seen(&lineage.flushed(2, A, failed())),
        [Seen::Failed(0, vec![second.seq])],
        "finality: nothing durable is reached back to"
    );
}

#[test]
fn a_failed_flush_reaches_the_files_whose_domains_intersect_its_own_and_every_unknown_file() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let writes = [
        ops.push_in(&mut lineage, 1, A, DEFAULT, &[1]),
        ops.push_in(&mut lineage, 1, B, DEFAULT, &[1, 2]),
        ops.push_in(&mut lineage, 1, C, DEFAULT, &[3]),
        ops.push(&mut lineage, 1, D, DEFAULT),
    ];
    for write in writes {
        lineage.completed(write, true);
    }
    assert_eq!(submitted(lineage.seal(1)).flushes.len(), 4);
    assert_eq!(
        seen(&lineage.flushed(1, A, failed())),
        [Seen::Failed(0, vec![0, 1, 3])],
        "B shares domain 1, D is unknown, C is disjoint"
    );
    assert_eq!(
        seen(&lineage.flushed(1, D, failed())),
        [Seen::Failed(1, vec![0, 1, 2, 3])],
        "an unknown file shares fate with every file"
    );
}

#[test]
fn an_import_reaches_its_scope_with_the_same_rule() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    ops.push_in(&mut lineage, 1, A, DEFAULT, &[1]);
    ops.push_in(&mut lineage, 1, B, DEFAULT, &[2]);
    ops.push(&mut lineage, 1, C, DEFAULT);
    let cases = [
        (domains(&[1]), vec![0, 2]),
        (domains(&[9]), vec![2]),
        (Reach::Nothing, vec![]),
        (Reach::All, vec![0, 1, 2]),
    ];
    for (seq, (reach, expected)) in cases.into_iter().enumerate() {
        let (id, due) = import(&mut lineage, &reach);
        assert_eq!(id.seq, seq as u64);
        assert_eq!(
            seen(&due),
            [Seen::Failed(seq as u64, expected)],
            "{reach:?}"
        );
    }
    let inventory = lineage.failures();
    assert_eq!(inventory.len(), 4, "in observation order");
    assert!(
        inventory
            .iter()
            .enumerate()
            .all(|(i, f)| f.id.seq == i as u64 && matches!(f.cause, Cause::Imported { .. }))
    );
}

#[test]
fn a_failure_holds_only_the_epochs_it_contains() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    ops.push(&mut lineage, 5, A, DEFAULT);
    let (_, due) = import(&mut lineage, &Reach::All);
    assert_eq!(seen(&due), [Seen::Failed(0, vec![0])]);
    assert_eq!(
        durable(&submitted(lineage.seal(3))),
        [3],
        "the failure holds epoch 5 alone"
    );

    let (_, due) = import(&mut lineage, &Reach::Nothing);
    assert_eq!(seen(&due), [Seen::Failed(1, vec![])]);
    assert_eq!(
        durable(&submitted(lineage.seal(4))),
        [4],
        "an empty suspect set holds nothing"
    );
}

#[test]
fn a_failure_observed_after_a_later_seal_finished_answers_it_blocked() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(first, true);
    lineage.completed(second, true);
    submitted(lineage.seal(1));
    submitted(lineage.seal(2));
    assert!(quiet(&lineage.flushed(2, B, Ok(()))));
    assert_eq!(
        seen(&lineage.flushed(1, A, failed())),
        [Seen::Failed(0, vec![first.seq]), Seen::Blocked(2, 0)]
    );
}

/// CONTRACT.md's "Healing a failure", without I/O.
#[test]
fn a_heal_takes_effect_at_the_first_seal_after_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    for file in [A, B, C] {
        let write = ops.push(&mut lineage, 41, file, DEFAULT);
        lineage.completed(write, true);
    }
    submitted(lineage.seal(41));
    let mut token = tokens(lineage.flushed(41, A, failed()));
    assert!(quiet(&lineage.flushed(41, B, Ok(()))));
    assert!(
        quiet(&lineage.flushed(41, C, Ok(()))),
        "41 was answered by its Failed"
    );
    assert_eq!(lineage.refuses(41), Some(41));
    for file in [A, B, C] {
        let write = ops.push(&mut lineage, 42, file, DEFAULT);
        lineage.completed(write, true);
    }

    let due = lineage
        .resolve(vec![(token.remove(0), Resolution::Heal)])
        .expect("heal");
    assert!(quiet(&due), "a heal waits for a seal");
    let id = lineage.failures()[0].id;
    assert!(
        lineage.failures()[0].token_live,
        "the pending heal holds the token"
    );
    assert!(lineage.take_token(id).is_none());

    assert_eq!(submitted(lineage.seal(42)).flushes.len(), 3);
    assert!(quiet(&lineage.flushed(42, A, Ok(()))));
    assert!(quiet(&lineage.flushed(42, B, Ok(()))));
    assert_eq!(durable(&lineage.flushed(42, C, Ok(()))), [41, 42]);
    assert!(
        lineage.failures().is_empty(),
        "a resolved failure leaves no memory"
    );
    assert_eq!(lineage.state(41), State::Durable);
}

#[test]
fn a_seal_made_before_the_heal_does_not_make_it_effective() {
    let mut ops = Ops::new();
    let (mut lineage, token) = one_failure(&mut ops);
    let write = ops.push(&mut lineage, 2, A, DEFAULT);
    lineage.completed(write, true);
    submitted(lineage.seal(2));
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    assert_eq!(
        seen(&lineage.flushed(2, A, Ok(()))),
        [Seen::Blocked(2, 0)],
        "seal 2 was made before the heal"
    );
    assert_eq!(durable(&submitted(lineage.seal(3))), [1, 2, 3]);
}

#[test]
fn a_heal_waits_for_a_seal_that_succeeds() {
    let mut ops = Ops::new();
    let (mut lineage, first) = one_failure(&mut ops);
    lineage
        .resolve(vec![(first, Resolution::Heal)])
        .expect("heal");
    let write = ops.push(&mut lineage, 2, A, DEFAULT);
    lineage.completed(write, true);
    submitted(lineage.seal(2));
    let due = lineage.flushed(2, A, failed());
    assert_eq!(
        seen(&due),
        [Seen::Failed(1, vec![0, write.seq])],
        "the first failure's write is still uncovered, so the second suspects it too"
    );
    let second = tokens(due).remove(0);
    assert_eq!(lineage.failures().len(), 2, "the heal did not take effect");

    lineage
        .resolve(vec![(second, Resolution::Heal)])
        .expect("heal");
    assert_eq!(durable(&submitted(lineage.seal(3))), [1, 2, 3]);
    assert!(lineage.failures().is_empty());
}

#[test]
fn abandoning_takes_effect_at_once_and_abandons_every_epoch_the_failure_contains() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let sealed = ops.push(&mut lineage, 1, A, DEFAULT);
    let open = ops.push(&mut lineage, 3, A, DEFAULT);
    lineage.completed(sealed, true);
    lineage.completed(open, true);
    submitted(lineage.seal(1));
    let token = tokens(lineage.flushed(1, A, failed())).remove(0);

    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(
        seen(&due),
        [Seen::Abandoned(0, vec![0, 1]), Seen::Durable(1)]
    );
    assert_eq!(lineage.state(1), State::Abandoned);
    assert_eq!(lineage.state(2), State::Open);
    assert_eq!(
        lineage.state(3),
        State::Abandoned,
        "decided now, though 3 is open"
    );
    assert!(lineage.failures().is_empty());
    assert_eq!(already(lineage.seal(1)), State::Abandoned);

    assert_eq!(submitted(lineage.seal(3)).flushes, [flush(3, A)]);
    assert_eq!(durable(&lineage.flushed(3, A, Ok(()))), [3]);
    assert_eq!(lineage.state(3), State::Abandoned);
}

/// CONTRACT.md's "Failures and epochs are many-to-many", second ending, without I/O.
#[test]
fn one_abandonment_abandons_an_epoch_that_still_waits_for_every_failure_containing_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let a = ops.push(&mut lineage, 41, A, DEFAULT);
    submitted(lineage.seal(41));
    let p = ops.push(&mut lineage, 43, A, DEFAULT);
    lineage.completed(a, true);
    lineage.completed(p, true);
    let due = lineage.flushed(41, A, failed());
    assert_eq!(seen(&due), [Seen::Failed(0, vec![a.seq, p.seq])]);
    let f1 = tokens(due).remove(0);
    let r = ops.push(&mut lineage, 43, A, DEFAULT);
    lineage.completed(r, true);
    let (_, due) = import(&mut lineage, &Reach::All);
    assert_eq!(seen(&due), [Seen::Failed(1, vec![a.seq, p.seq, r.seq])]);
    let f2 = tokens(due).remove(0);

    let due = lineage
        .resolve(vec![(f1, Resolution::Heal), (f2, Resolution::Abandon)])
        .expect("resolve both");
    assert_eq!(
        seen(&due),
        [Seen::Abandoned(1, vec![a.seq, p.seq, r.seq])],
        "41 still sits in failure 0, whose heal waits for a seal"
    );
    assert_eq!(lineage.state(41), State::Abandoned);
    assert_eq!(lineage.state(43), State::Abandoned);

    assert_eq!(submitted(lineage.seal(44)).flushes, [flush(44, A)]);
    assert_eq!(durable(&lineage.flushed(44, A, Ok(()))), [41, 44]);
    assert_eq!(lineage.state(41), State::Abandoned);
}

#[test]
fn a_refused_resolution_changes_nothing_and_hands_every_token_back() {
    let mut ops = Ops::new();
    let (mut lineage, token) = one_failure(&mut ops);
    let foreign_id = FailureId {
        instance: InstanceId::next(),
        seq: 0,
    };
    let (foreign, _) = FailureToken::mint(foreign_id);
    let error = lineage
        .resolve(vec![
            (token, Resolution::Abandon),
            (foreign, Resolution::Heal),
        ])
        .expect_err("a foreign token");
    assert!(matches!(error.reason, ResolveRefusal::Foreign(id) if id == foreign_id));
    let returned: Vec<(u64, Resolution)> = error
        .returned
        .iter()
        .map(|(token, resolution)| (token.id().seq, *resolution))
        .collect();
    assert_eq!(
        returned,
        [(0, Resolution::Abandon), (0, Resolution::Heal)],
        "in the order given"
    );
    assert_eq!(lineage.state(1), State::Blocked(lineage.failures()[0].id));
    assert!(lineage.failures()[0].token_live, "the token came back live");
}

#[test]
fn the_inventory_hands_out_a_token_only_when_none_is_live() {
    let mut ops = Ops::new();
    let (mut lineage, token) = one_failure(&mut ops);
    let id = token.id();
    assert!(lineage.take_token(id).is_none(), "the Failed token is live");
    drop(token);
    assert!(!lineage.failures()[0].token_live, "dropping it is close");
    let again = lineage.take_token(id).expect("a closed failure's token");
    assert!(lineage.take_token(id).is_none());
    again.close();
    let third = lineage.take_token(id).expect("closed again");
    assert!(
        lineage
            .take_token(FailureId {
                instance: InstanceId::next(),
                seq: id.seq,
            })
            .is_none(),
        "another instance's identity"
    );
    lineage
        .resolve(vec![(third, Resolution::Heal)])
        .expect("heal");
    assert!(
        lineage.take_token(id).is_none(),
        "the pending heal holds it"
    );
    assert_eq!(lineage.failures().len(), 1, "unresolved until its seal");
}
