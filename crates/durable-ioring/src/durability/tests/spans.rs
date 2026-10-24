// Copyright (c) 2026 Mike Grier
//! The span entries a seal's work on a file is charged to (DI-D-98, DI-D-107): made by the first
//! write to a file in a span, divided when a seal names some of a file's writes and not the rest,
//! and released with the last write to leave. Every change here is also checked, inside the core,
//! against a census of the uncovered writes (`spans_agree`).

use std::io;

use super::{
    A, B, DEFAULT, FULL, Ops, Seen, State, durable, failed, flush, inconsistent, lineage, quiet,
    seen, submitted,
};
use crate::domains::DomainId;
use crate::durability::{DEFAULT_LINEAGE, Routing};

/// A file only the provider answers for.
const PROVIDER: Routing = Routing {
    default: false,
    provider: true,
};

#[test]
fn a_first_write_to_a_file_makes_an_entry_and_later_ones_share_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    assert_eq!(lineage.span_entries(), 0);
    ops.push(&mut lineage, 1, A, DEFAULT);
    assert_eq!(lineage.span_entries(), 1);
    ops.push(&mut lineage, 1, A, DEFAULT);
    ops.push(&mut lineage, 4, A, DEFAULT);
    assert_eq!(
        lineage.span_entries(),
        1,
        "the same file, the same unsealed span"
    );
}

#[test]
fn each_file_and_each_lineage_has_its_own_entry() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    ops.push(&mut lineage, 1, A, DEFAULT);
    ops.push(&mut lineage, 1, B, DEFAULT);
    ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    assert_eq!(lineage.span_entries(), 3);
}

#[test]
fn a_seal_naming_every_write_of_a_file_moves_its_entry() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let writes = [
        ops.push(&mut lineage, 1, A, DEFAULT),
        ops.push(&mut lineage, 2, A, DEFAULT),
    ];
    for write in writes {
        lineage.completed(write, FULL);
    }
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, A)]);
    assert_eq!(
        lineage.span_entries(),
        1,
        "moved to the seal's span, not copied"
    );
}

#[test]
fn a_seal_dividing_a_files_writes_makes_an_entry_for_each_span() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let low = ops.push(&mut lineage, 3, A, DEFAULT);
    let high = ops.push(&mut lineage, 7, A, DEFAULT);
    lineage.completed(low, FULL);
    lineage.completed(high, FULL);
    assert_eq!(lineage.span_entries(), 1);
    assert_eq!(submitted(lineage.seal(5)).flushes, [flush(5, A)]);
    assert_eq!(
        lineage.span_entries(),
        2,
        "the seal's span holds the write at 3, the unsealed span the one at 7"
    );
    assert_eq!(durable(&lineage.flushed(5, A, Ok(()))), [5]);
    assert_eq!(
        lineage.span_entries(),
        1,
        "the seal's entry left with its covered write"
    );
    assert_eq!(submitted(lineage.seal(7)).flushes, [flush(7, A)]);
    assert_eq!(durable(&lineage.flushed(7, A, Ok(()))), [7]);
    assert_eq!(lineage.span_entries(), 0);
}

#[test]
fn a_successful_flush_releases_the_entry_with_its_covered_writes() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let writes = [
        ops.push(&mut lineage, 1, A, DEFAULT),
        ops.push(&mut lineage, 1, B, DEFAULT),
    ];
    for write in writes {
        lineage.completed(write, FULL);
    }
    submitted(lineage.seal(1));
    assert_eq!(lineage.span_entries(), 2);
    assert!(quiet(&lineage.flushed(1, B, Ok(()))), "A has not answered");
    assert_eq!(
        lineage.span_entries(),
        1,
        "B's entry is released, A's is not"
    );
    assert_eq!(durable(&lineage.flushed(1, A, Ok(()))), [1]);
    assert_eq!(lineage.span_entries(), 0);
}

#[test]
fn a_failed_write_leaves_its_entry_which_goes_with_the_last() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(first, failed());
    assert_eq!(lineage.span_entries(), 1, "the second write still holds it");
    lineage.completed(second, failed());
    assert_eq!(lineage.span_entries(), 0);
}

#[test]
fn a_failed_flush_keeps_its_entry_while_its_writes_are_uncovered() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    submitted(lineage.seal(1));
    let due = lineage.flushed(1, A, Err(io::Error::from_raw_os_error(1117)));
    assert!(
        seen(&due)
            .iter()
            .any(|seen| matches!(seen, Seen::Failed(..))),
        "the failed flush is observed"
    );
    assert!(matches!(lineage.state(1), State::Blocked(_)));
    assert_eq!(lineage.span_entries(), 1, "the failure suspects the write");
}

#[test]
fn ending_a_lineage_releases_its_entries_but_a_write_in_flight_keeps_its_own() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let done = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    let flying = ops.push_to(&mut lineage, minted, 2, B, DEFAULT, &[]);
    lineage.completed(done, FULL);
    assert_eq!(lineage.span_entries(), 2);
    lineage.end(minted);
    assert_eq!(lineage.span_entries(), 1, "B's write is still in flight");
    lineage.completed(flying, FULL);
    assert_eq!(lineage.span_entries(), 0, "let go as it completes");
}

#[test]
fn an_ended_lineage_keeps_the_entry_of_a_flush_out_until_it_answers() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let on_a = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    let on_b = ops.push_to(&mut lineage, minted, 2, B, DEFAULT, &[]);
    lineage.completed(on_a, FULL);
    lineage.completed(on_b, FULL);
    submitted(lineage.0.seal(minted, 1));
    assert_eq!(lineage.span_entries(), 2, "A's in the seal, B's unsealed");
    lineage.end(minted);
    assert_eq!(lineage.span_entries(), 1, "only the entry of A's flush out");
    lineage.0.flushed(minted, 1, A, Ok(()));
    assert_eq!(lineage.span_entries(), 0);
}

#[test]
fn an_ended_lineage_lets_go_the_entry_of_a_flush_that_has_answered() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let minted = lineage.mint(None);
    let on_a = ops.push_to(&mut lineage, minted, 1, A, DEFAULT, &[]);
    let on_b = ops.push_to(&mut lineage, minted, 2, B, DEFAULT, &[]);
    lineage.completed(on_a, FULL);
    lineage.completed(on_b, FULL);
    submitted(lineage.0.seal(minted, 2));
    lineage.0.flushed(
        minted,
        2,
        A,
        Err(super::fault(io::Error::from_raw_os_error(1117))),
    );
    assert_eq!(lineage.span_entries(), 2, "the failure suspects A's write");
    lineage.end(minted);
    assert_eq!(lineage.span_entries(), 1, "A's flush answered; B's is out");
}

#[test]
fn an_ended_lineage_lets_go_the_entry_of_a_file_no_flush_is_out_for() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    lineage.serve(
        std::iter::once(DomainId(9)).collect(),
        std::vec![crate::types::FlushDomain::new(b"d9".to_vec())],
    );
    let minted = lineage.mint(None);
    let write = ops.push_to(&mut lineage, minted, 1, A, PROVIDER, &[9]);
    lineage.completed(write, FULL);
    let due = submitted(lineage.0.seal(minted, 1));
    assert!(due.flushes.is_empty(), "the provider answers for it");
    assert_eq!(lineage.span_entries(), 1);
    lineage.end(minted);
    assert_eq!(lineage.span_entries(), 0, "no commit of it is out");
}

#[test]
fn a_file_only_the_provider_answers_for_has_an_entry_and_no_flush() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    lineage.serve(
        std::iter::once(DomainId(9)).collect(),
        std::vec![crate::types::FlushDomain::new(b"d9".to_vec())],
    );
    let write = ops.push_in(&mut lineage, 1, A, PROVIDER, &[9]);
    assert_eq!(lineage.span_entries(), 1);
    lineage.completed(write, FULL);
    let due = submitted(lineage.seal(1));
    assert!(due.flushes.is_empty(), "the provider answers for it");
    assert_eq!(lineage.span_entries(), 1);
    let due = lineage.provided(DEFAULT_LINEAGE, 0, DomainId(9), Ok(()));
    assert_eq!(durable(&due), [1]);
    assert_eq!(lineage.span_entries(), 0);
}

#[test]
fn two_seals_flushing_one_file_answer_through_their_own_entries() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let low = ops.push(&mut lineage, 1, A, DEFAULT);
    let high = ops.push(&mut lineage, 2, A, DEFAULT);
    lineage.completed(low, FULL);
    lineage.completed(high, FULL);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, A)]);
    assert_eq!(lineage.span_entries(), 2);
    assert!(
        quiet(&lineage.flushed(2, A, Ok(()))),
        "1 is not yet durable"
    );
    assert_eq!(
        lineage.span_entries(),
        1,
        "the later seal's entry went with its write"
    );
    assert_eq!(durable(&lineage.flushed(1, A, Ok(()))), [1, 2]);
    assert_eq!(lineage.span_entries(), 0);
}

#[test]
fn there_are_never_more_entries_than_uncovered_writes() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let files = [A, B];
    let mut pushed = std::vec::Vec::new();
    for epoch in 1..=12_u64 {
        let file = files[usize::from(epoch % 3 == 0)];
        pushed.push(ops.push(&mut lineage, epoch, file, DEFAULT));
        assert!(lineage.span_entries() <= pushed.len());
    }
    for write in &pushed {
        lineage.completed(*write, FULL);
    }
    for through in [2_u64, 5, 9, 12] {
        let flushes = submitted(lineage.seal(through)).flushes;
        assert!(lineage.span_entries() <= pushed.len());
        for flushed in flushes {
            lineage.flushed(through, flushed.file, Ok(()));
        }
    }
    assert_eq!(lineage.durable_through(), Some(12));
    assert_eq!(lineage.span_entries(), 0);
}

#[test]
fn a_flush_answered_again_while_its_entry_remains_is_an_inconsistency() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 1, B, DEFAULT);
    lineage.completed(first, FULL);
    lineage.completed(second, FULL);
    submitted(lineage.seal(1));
    // A failed flush leaves its write uncovered, so its entry remains to be answered again.
    lineage.flushed(1, A, Err(io::Error::from_raw_os_error(1117)));
    assert_eq!(lineage.span_entries(), 2);
    inconsistent("a flush completed that the seal did not issue", || {
        lineage.flushed(1, A, Ok(()))
    });
}

#[test]
fn a_flush_answered_again_after_its_entry_was_released_is_an_inconsistency() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 1, B, DEFAULT);
    lineage.completed(first, FULL);
    lineage.completed(second, FULL);
    submitted(lineage.seal(1));
    lineage.flushed(1, A, Ok(()));
    assert_eq!(
        lineage.span_entries(),
        1,
        "A's entry went with its covered write"
    );
    inconsistent("a flush completed that the seal did not issue", || {
        lineage.flushed(1, A, Ok(()))
    });
}
