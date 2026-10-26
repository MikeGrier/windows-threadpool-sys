// Copyright (c) 2026 Mike Grier
//! When a heal takes effect in a lineage (DI-D-41, DI-D-46, DI-D-114): at the first seal made after
//! it that finishes successfully and reaches every write of the lineage pushed before it that is
//! still to be committed. Nothing is kept for each lineage a heal waits on: what it waits for is
//! read from the write records when a seal finishes, so a write that has nothing left to commit --
//! it failed, or a seal covered it -- is not waited for.

use std::io;

use super::{A, B, C, DEFAULT, FULL, Lineage, Ops, Seen, failed, lineage, seen, submitted, tokens};
use crate::durability::Reach;
use crate::ids::FailureToken;
use crate::types::{FailureCause, ImportScope, Resolution};

/// A failure the consumer learned of, reaching every uncovered write: its token.
fn import(lineage: &mut Lineage) -> FailureToken {
    let cause = FailureCause::Imported {
        scope: ImportScope::All,
    };
    tokens(lineage.import(cause, &Reach::All).1).remove(0)
}

fn heal(lineage: &mut Lineage, token: FailureToken) {
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
}

/// Whether failure 0's `Healed` is among `seen`.
fn healed(seen: &[Seen]) -> bool {
    seen.iter().any(|seen| matches!(seen, Seen::Healed(0, ..)))
}

/// The common case, and the rule DI-D-41 states: a seal stopping below a write re-issued before
/// the heal does not release it, and the first that reaches it does.
#[test]
fn a_heal_waits_for_a_seal_reaching_every_write_pushed_before_it_still_to_commit() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let suspect = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(suspect, FULL);
    let token = import(&mut lineage);
    let again = ops.push(&mut lineage, 2, A, DEFAULT);
    let higher = ops.push(&mut lineage, 3, A, DEFAULT);
    lineage.completed(again, FULL);
    lineage.completed(higher, FULL);
    heal(&mut lineage, token);
    submitted(lineage.seal(2));
    let answered = seen(&lineage.flushed(2, A, Ok(())));
    assert!(!healed(&answered), "3 is still to commit: {answered:?}");
    submitted(lineage.seal(3));
    let answered = seen(&lineage.flushed(3, A, Ok(())));
    assert!(healed(&answered), "{answered:?}");
}

/// A write pushed before the heal that failed has nothing to commit: a seal stopping below it
/// releases the heal. The consumer learned of it from its own completion (DI-D-113).
#[test]
fn a_write_that_failed_is_not_waited_for() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let suspect = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(suspect, FULL);
    let token = import(&mut lineage);
    let again = ops.push(&mut lineage, 2, A, DEFAULT);
    let higher = ops.push(&mut lineage, 3, A, DEFAULT);
    lineage.completed(again, FULL);
    lineage.completed(higher, failed());
    heal(&mut lineage, token);
    submitted(lineage.seal(2));
    let answered = seen(&lineage.flushed(2, A, Ok(())));
    assert!(
        healed(&answered),
        "3's only write failed, so nothing at 3 is to commit: {answered:?}"
    );
}

/// A write pushed before the heal that a later seal covered -- its file's flush succeeded, though
/// the seal failed on another file -- has nothing to commit either: an earlier seal finishing after
/// that coverage releases the heal though it stops below the write.
#[test]
fn a_write_a_failed_seal_covered_is_not_waited_for() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let suspect = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(suspect, FULL);
    let token = import(&mut lineage);
    let on_a = ops.push(&mut lineage, 2, A, DEFAULT);
    let on_b = ops.push(&mut lineage, 3, B, DEFAULT);
    lineage.completed(on_a, FULL);
    lineage.completed(on_b, FULL);
    heal(&mut lineage, token);
    // Pushed after the heal, so not what the heal waits for: the seal through 3 fails on its file.
    let on_c = ops.push(&mut lineage, 3, C, DEFAULT);
    lineage.completed(on_c, FULL);
    submitted(lineage.seal(2));
    submitted(lineage.seal(3));
    lineage.flushed(3, B, Ok(()));
    lineage.flushed(3, C, Err(io::Error::from_raw_os_error(1117)));
    let answered = seen(&lineage.flushed(2, A, Ok(())));
    assert!(
        healed(&answered),
        "B's write at 3 is covered, and C's was pushed after the heal: {answered:?}"
    );
}

/// Heals made in turn are released in turn: a seal that reaches what the later one waits for
/// releases both, and one that reaches only the earlier's releases only it.
#[test]
fn a_seal_releases_every_heal_made_before_the_one_it_releases() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let suspect = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(suspect, FULL);
    let first = import(&mut lineage);
    let second = import(&mut lineage);
    heal(&mut lineage, first);
    let later = ops.push(&mut lineage, 2, A, DEFAULT);
    lineage.completed(later, FULL);
    heal(&mut lineage, second);
    submitted(lineage.seal(1));
    let answered = seen(&lineage.flushed(1, A, Ok(())));
    assert!(
        healed(&answered),
        "the first waited for nothing above 1: {answered:?}"
    );
    assert!(
        !answered
            .iter()
            .any(|seen| matches!(seen, Seen::Healed(1, ..))),
        "the second waits for 2: {answered:?}"
    );
    submitted(lineage.seal(2));
    let answered = seen(&lineage.flushed(2, A, Ok(())));
    assert!(
        answered
            .iter()
            .any(|seen| matches!(seen, Seen::Healed(1, ..))),
        "{answered:?}"
    );
}
