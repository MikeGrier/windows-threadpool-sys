// Copyright (c) 2026 Mike Grier
//! What the core holds of one file (DI-3.2.7): the part of `remove_target`'s refusal it answers.

use std::io;

use super::{A, B, DEFAULT, FULL, Lineage, Ops, lineage, submitted, tokens};
use crate::dioring::WinInstant;
use crate::dioring::durability::files::FileHolds;
use crate::dioring::durability::{DEFAULT_LINEAGE, Key, Reach};
use crate::types::{Epoch, FailureCause, ImportScope, IoTargetKey, Resolution};

fn holds(lineage: &Lineage, file: IoTargetKey) -> FileHolds<u64, WinInstant> {
    lineage.file_holds(file)
}

/// Epoch `epoch` of the lineage keyed `key`, as a hold names it.
fn at(lineage: &Lineage, key: Key, epoch: u64) -> Epoch<crate::ids::DioringIds<u64>> {
    Epoch::new(lineage.0.name(key), epoch)
}

fn none() -> FileHolds<u64, WinInstant> {
    FileHolds {
        uncovered: 0,
        uncovered_through: Vec::new(),
        failures: Vec::new(),
    }
}

fn failed() -> io::Result<()> {
    Err(io::Error::from_raw_os_error(1117))
}

#[test]
fn a_file_nothing_was_written_to_is_held_by_nothing() {
    let lineage = lineage();
    assert_eq!(holds(&lineage, A), none());
    assert!(holds(&lineage, A).is_empty());
}

#[test]
fn a_write_in_flight_is_the_relays_to_count() {
    let mut lineage = lineage();
    Ops::new().push(&mut lineage, 1, A, DEFAULT);
    assert_eq!(holds(&lineage, A), none());
}

#[test]
fn a_completed_write_holds_its_file_until_it_is_covered() {
    let mut lineage = lineage();
    let write = Ops::new().push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    let expected = FileHolds {
        uncovered: 1,
        uncovered_through: vec![at(&lineage, DEFAULT_LINEAGE, 1)],
        failures: Vec::new(),
    };
    assert_eq!(holds(&lineage, A), expected);
    assert!(!holds(&lineage, A).is_empty());

    submitted(lineage.seal(1));
    assert_eq!(
        holds(&lineage, A),
        expected,
        "named to a seal whose flush has not answered"
    );
    lineage.flushed(1, A, Ok(()));
    assert_eq!(holds(&lineage, A), none(), "covered");
}

#[test]
fn the_highest_uncovered_epoch_is_reported_per_lineage_in_lineage_order() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let writes = [
        ops.push_to(&mut lineage, minted, 2, A, DEFAULT, &[]),
        ops.push(&mut lineage, 3, A, DEFAULT),
        ops.push(&mut lineage, 1, A, DEFAULT),
        ops.push(&mut lineage, 5, B, DEFAULT),
    ];
    for write in writes {
        lineage.completed(write, FULL);
    }
    assert_eq!(
        holds(&lineage, A),
        FileHolds {
            uncovered: 3,
            uncovered_through: vec![at(&lineage, DEFAULT_LINEAGE, 3), at(&lineage, minted, 2)],
            failures: Vec::new(),
        },
        "B's write is B's"
    );
    assert_eq!(holds(&lineage, B).uncovered, 1);
}

#[test]
fn a_suspect_write_is_reported_through_its_failure_and_not_as_uncovered() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let on_a = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(on_a, FULL);
    submitted(lineage.seal(1));
    let token = tokens(lineage.flushed(1, A, failed())).remove(0);
    let later = ops.push(&mut lineage, 2, A, DEFAULT);
    lineage.completed(later, FULL);
    assert_eq!(
        holds(&lineage, A),
        FileHolds {
            uncovered: 1,
            uncovered_through: vec![at(&lineage, DEFAULT_LINEAGE, 2)],
            failures: vec![token.id()],
        },
        "the write pushed after the failure is uncovered, the one before it suspect"
    );

    lineage
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("abandon");
    assert!(holds(&lineage, A).failures.is_empty(), "resolved");
}

#[test]
fn a_failure_holds_a_file_whose_suspect_write_was_covered_until_it_is_resolved() {
    let mut lineage = lineage();
    let write = Ops::new().push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    let (id, _) = lineage.import(
        FailureCause::Imported {
            scope: ImportScope::All,
        },
        &Reach::All,
    );
    submitted(lineage.seal(1));
    lineage.flushed(1, A, Ok(()));
    assert_eq!(
        holds(&lineage, A),
        FileHolds {
            uncovered: 0,
            uncovered_through: Vec::new(),
            failures: vec![id],
        },
        "covered, and still in the failure's suspect set"
    );
}

#[test]
fn a_failure_suspecting_only_other_files_does_not_hold_this_one() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let on_a = ops.push_in(&mut lineage, 1, A, DEFAULT, &[1]);
    let on_b = ops.push_in(&mut lineage, 1, B, DEFAULT, &[2]);
    lineage.completed(on_a, FULL);
    lineage.completed(on_b, FULL);
    submitted(lineage.seal(1));
    let id = tokens(lineage.flushed(1, A, failed())).remove(0).id();
    assert_eq!(holds(&lineage, A).failures, [id]);
    assert_eq!(
        holds(&lineage, B),
        FileHolds {
            uncovered: 1,
            uncovered_through: vec![at(&lineage, DEFAULT_LINEAGE, 1)],
            failures: Vec::new(),
        },
        "B's domain is disjoint from A's"
    );
}

#[test]
fn an_ended_lineages_completed_writes_no_longer_hold_their_file() {
    let mut lineage = lineage();
    let minted = lineage.mint(None);
    let write = Ops::new().push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    lineage.completed(write, FULL);
    assert_eq!(holds(&lineage, A).uncovered, 1);
    lineage.end(minted);
    assert_eq!(holds(&lineage, A), none());
}
