// Copyright (c) 2026 Mike Grier
//! Markings (DI-D-36, DI-D-39), without a ring: what happens to a failure's suspect writes after
//! it is observed is appended to its record and reported, and its final record -- `Abandoned` or
//! `Healed` -- carries them.

use std::io;

use super::{
    A, B, DEFAULT, Due, FAILED, FULL, IO_DEVICE, Lineage, One, Ops, Seen, flush, lineage, seen,
    submitted, tokens,
};
use crate::dioring::durability::{Reach, WriteEnd};
use crate::dioring::tests::MockClock;
use crate::ids::{InstanceId, Lineage as LineageId};
use crate::types::{Cause, ImportScope, MarkingKind, Outcome, Resolution};

const NULLIFIED: MarkingKind = MarkingKind::Nullified {
    code: Some(IO_DEVICE),
};
const SHORT: MarkingKind = MarkingKind::Short { transferred: 3 };

fn import(lineage: &mut Lineage) -> Due<u64, &'static str> {
    let cause = Cause::Imported {
        scope: ImportScope::All,
    };
    lineage.import(cause, &Reach::All).1
}

/// The inventory's markings for its first failure, by write and kind.
fn inventory(lineage: &Lineage) -> Vec<(u64, MarkingKind)> {
    lineage.failures()[0]
        .markings
        .iter()
        .map(|m| (m.write.seq, m.kind))
        .collect()
}

#[test]
fn a_suspect_write_that_completes_failed_is_nullified_with_its_code() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    import(&mut lineage);
    assert_eq!(
        seen(&lineage.completed(write, FAILED)),
        [Seen::Marked(0, write.seq, NULLIFIED)]
    );
    assert_eq!(inventory(&lineage), [(write.seq, NULLIFIED)]);
}

#[test]
fn a_write_that_failed_before_the_failure_was_observed_is_neither_suspected_nor_marked() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FAILED);
    assert_eq!(seen(&import(&mut lineage)), [Seen::Failed(0, vec![])]);
    assert!(inventory(&lineage).is_empty());
}

#[test]
fn a_suspect_write_that_completes_in_full_is_not_marked() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    import(&mut lineage);
    assert!(seen(&lineage.completed(write, FULL)).is_empty());
}

#[test]
fn a_suspect_write_that_completes_short_is_marked_short_and_stays_held() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    import(&mut lineage);
    assert_eq!(
        seen(&lineage.completed(write, WriteEnd::Transferred(3))),
        [Seen::Marked(0, write.seq, SHORT)]
    );
    assert_eq!(
        submitted(lineage.seal(1)).flushes,
        [flush(1, A)],
        "a short write is still a completed write, named to its seal"
    );
}

#[test]
fn a_zero_byte_completion_is_short() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    import(&mut lineage);
    assert_eq!(
        seen(&lineage.completed(write, WriteEnd::Transferred(0))),
        [Seen::Marked(
            0,
            write.seq,
            MarkingKind::Short { transferred: 0 }
        )]
    );
}

#[test]
fn a_suspect_write_whose_own_file_is_flushed_successfully_is_marked_covered() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    import(&mut lineage);
    submitted(lineage.seal(1));
    assert_eq!(
        seen(&lineage.flushed(1, A, Ok(()))),
        [
            Seen::Marked(0, write.seq, MarkingKind::Covered),
            Seen::Blocked(1, 0)
        ],
        "covered, and still held by the failure: a marking changes nothing the mark depends on"
    );
}

/// A write is covered by its own file's flush, not by its seal: here the seal fails on another
/// file, and the write's bytes are on the device all the same.
#[test]
fn a_flush_failure_on_one_file_leaves_the_writes_another_file_flushed_covered() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let on_a = ops.push(&mut lineage, 1, A, DEFAULT);
    let on_b = ops.push(&mut lineage, 1, B, DEFAULT);
    lineage.completed(on_a, FULL);
    lineage.completed(on_b, FULL);
    submitted(lineage.seal(1));
    let failed = lineage.flushed(1, A, Err(io::Error::from_raw_os_error(1117)));
    assert_eq!(seen(&failed), [Seen::Failed(0, vec![on_a.seq, on_b.seq])]);
    assert_eq!(
        seen(&lineage.flushed(1, B, Ok(()))),
        [Seen::Marked(0, on_b.seq, MarkingKind::Covered)]
    );
    assert_eq!(inventory(&lineage), [(on_b.seq, MarkingKind::Covered)]);
}

#[test]
fn a_write_two_failures_suspect_marks_both_with_one_reading() {
    let clock = MockClock::at(900);
    let mut lineage = One::with_clock(clock.clone());
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    let cause = || Cause::Imported {
        scope: ImportScope::All,
    };
    lineage.import(cause(), &Reach::All);
    lineage.import(cause(), &Reach::All);
    clock.advance(100);
    let due = lineage.completed(write, FAILED);
    assert_eq!(
        seen(&due),
        [
            Seen::Marked(0, write.seq, NULLIFIED),
            Seen::Marked(1, write.seq, NULLIFIED)
        ]
    );
    let stamps: Vec<u64> = lineage
        .failures()
        .iter()
        .map(|failure| failure.markings[0].observed.ticks())
        .collect();
    assert_eq!(stamps, [1_000, 1_000]);
}

#[test]
fn abandoning_carries_the_markings_and_ends_them() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 1, A, DEFAULT);
    let token = tokens(import(&mut lineage)).remove(0);
    lineage.completed(first, FAILED);
    let due = lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert_eq!(
        seen(&due),
        [Seen::Abandoned(
            0,
            vec![first.seq, second.seq],
            vec![(first.seq, NULLIFIED)]
        )]
    );
    assert!(
        seen(&lineage.completed(second, WriteEnd::Transferred(3))).is_empty(),
        "an abandoned failure gains no marking"
    );
}

/// A heal that takes effect reports the failure's final record, before any `Durable` it lets
/// through; a healing failure goes on gaining markings until then.
#[test]
fn a_heal_taking_effect_reports_healed_with_the_record_before_any_durable() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let sealed = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(sealed, FULL);
    submitted(lineage.seal(1));
    let later = ops.push(&mut lineage, 2, A, DEFAULT);
    let token = tokens(lineage.flushed(1, A, Err(io::Error::from_raw_os_error(1117)))).remove(0);
    lineage.completed(later, WriteEnd::Transferred(3));
    lineage
        .resolve(vec![(token, Resolution::Heal)])
        .expect("heal");
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, A)]);
    assert_eq!(
        seen(&lineage.flushed(2, A, Ok(()))),
        [
            Seen::Marked(0, later.seq, MarkingKind::Covered),
            Seen::Healed(
                0,
                vec![sealed.seq, later.seq],
                vec![(later.seq, SHORT), (later.seq, MarkingKind::Covered)]
            ),
            Seen::Durable(1),
            Seen::Durable(2),
        ]
    );
    assert!(lineage.failures().is_empty());
}

#[test]
fn the_inventory_reports_markings_as_they_accrue() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 1, A, DEFAULT);
    import(&mut lineage);
    assert!(inventory(&lineage).is_empty());
    lineage.completed(first, WriteEnd::Transferred(3));
    assert_eq!(inventory(&lineage), [(first.seq, SHORT)]);
    lineage.completed(second, FAILED);
    assert_eq!(
        inventory(&lineage),
        [(first.seq, SHORT), (second.seq, NULLIFIED)]
    );
}

#[test]
fn a_completion_ends_a_write_as_its_outcome_says() {
    type O = Outcome<crate::ids::DioringIds<u64>>;
    assert_eq!(
        WriteEnd::of(&O::Transferred(5)),
        Some(WriteEnd::Transferred(5))
    );
    assert_eq!(
        WriteEnd::of(&O::Failed(io::Error::from_raw_os_error(1117))),
        Some(WriteEnd::Failed(Some(IO_DEVICE)))
    );
    assert_eq!(
        WriteEnd::of(&O::Failed(io::Error::other("no code"))),
        Some(WriteEnd::Failed(None))
    );
    let lineage = LineageId {
        instance: InstanceId::next(),
        seq: 0,
    };
    assert_eq!(
        WriteEnd::of(&O::NeverIssued {
            abandoned: crate::types::Epoch::new(lineage, 1)
        }),
        None
    );
}
