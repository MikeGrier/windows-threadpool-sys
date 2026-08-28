// Copyright (c) Mike Grier.

//! The asserted tier.
//!
//! Each test pins a measured platform fact that a design decision rests on, so
//! that a platform change falsifies the build rather than the design note. Facts
//! that cannot be asserted safely -- because they hang, cost seconds, mutate the
//! process irreversibly, or need a specific environment -- are not here; see the
//! crate documentation for which tier each probe belongs to and why.

use crate::error_mode::{
    bits, combined_invalid_installs_nothing, probe_bit, settable_bits,
    thread_mode_independent_of_process,
};
use crate::handle_state::{
    Fixture, SingleShot, closing_duplicate_preserves_source, duplicate_shares_cursor, ground_truth,
    query_disturbs_cursor, separate_opens_are_independent,
};

#[test]
fn the_three_documented_bits_are_settable_per_thread() {
    for bit in [
        bits::FAIL_CRITICAL_ERRORS,
        bits::NO_GP_FAULT_ERROR_BOX,
        bits::NO_OPEN_FILE_ERROR_BOX,
    ] {
        let outcome = probe_bit(bit);
        assert!(
            outcome.is_settable(),
            "0x{bit:04X} should be settable per thread, got {outcome:?}"
        );
    }
}

#[test]
fn the_alignment_bit_is_rejected_rather_than_silently_dropped() {
    let outcome = probe_bit(bits::NO_ALIGNMENT_FAULT_EXCEPT);
    assert!(
        !outcome.set_ok,
        "the alignment bit should be refused outright, got {outcome:?}"
    );
    // The distinction this whole probe exists for: a bit accepted and then
    // dropped would let a caller believe it installed a value it did not.
    assert!(
        !outcome.is_silently_dropped(),
        "the alignment bit was accepted but not installed, which is worse than \
         rejection: {outcome:?}"
    );
}

#[test]
fn the_settable_mask_excludes_only_the_alignment_bit() {
    let expected =
        bits::FAIL_CRITICAL_ERRORS | bits::NO_GP_FAULT_ERROR_BOX | bits::NO_OPEN_FILE_ERROR_BOX;
    assert_eq!(settable_bits(), expected, "the settable mask moved");
}

#[test]
fn one_invalid_bit_costs_the_caller_every_valid_bit() {
    let (installed_nothing, read_back) = combined_invalid_installs_nothing();
    assert!(
        installed_nothing,
        "expected an invalid bit to fail the whole call, but the valid bits \
         survived: read back 0x{read_back:04X}"
    );
}

#[test]
fn the_thread_error_mode_is_not_a_view_of_the_process_mode() {
    let observation = thread_mode_independent_of_process();
    assert!(
        observation.is_independent(),
        "a process-scope bit showed through the thread mode, which would let \
         capture observe a value a declarable type cannot hold: {observation:?}"
    );
}

#[test]
fn a_duplicated_handle_continues_the_sources_enumeration() {
    let fixture = Fixture::new("dup-shares");
    let truth = ground_truth(&fixture);
    let observation = duplicate_shares_cursor(&fixture);
    assert!(
        observation.continued(&truth),
        "a duplicate should share the enumeration cursor: {observation:?}"
    );
}

#[test]
fn a_separate_open_does_not_continue_another_handles_enumeration() {
    // The control. Without it, "the duplicate continued" could mean any handle
    // continues, which would say nothing about duplication at all.
    let fixture = Fixture::new("separate-opens");
    let observation = separate_opens_are_independent(&fixture);
    assert!(
        observation.restarted(),
        "a separate open should have its own cursor: {observation:?}"
    );
}

#[test]
fn closing_a_duplicate_leaves_the_source_usable() {
    let fixture = Fixture::new("close-dup");
    assert!(
        closing_duplicate_preserves_source(&fixture),
        "closing a duplicate broke the source handle, which would make a \
         request that owns a duplicate unsafe to drop"
    );
}

#[test]
fn single_shot_queries_do_not_disturb_an_enumeration_in_progress() {
    let fixture = Fixture::new("interleave");
    let truth = ground_truth(&fixture);
    for query in [SingleShot::BasicInfo, SingleShot::IdInfo, SingleShot::NonEx] {
        let (succeeded, disturbed) = query_disturbs_cursor(&fixture, query, false, &truth);
        assert!(
            succeeded,
            "{query:?} did not succeed, so it measured nothing"
        );
        assert!(!disturbed, "{query:?} moved the enumeration cursor");
    }
}

#[test]
fn a_query_on_a_duplicate_does_not_disturb_the_sources_enumeration() {
    let fixture = Fixture::new("interleave-dup");
    let truth = ground_truth(&fixture);
    let (succeeded, disturbed) =
        query_disturbs_cursor(&fixture, SingleShot::BasicInfo, true, &truth);
    assert!(
        succeeded,
        "the query did not succeed, so it measured nothing"
    );
    assert!(
        !disturbed,
        "a query on a duplicate moved the source's enumeration cursor"
    );
}
