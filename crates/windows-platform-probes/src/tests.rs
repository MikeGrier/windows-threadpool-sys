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
use std::time::Duration;

use windows_sys::Win32::Foundation::ERROR_NO_TOKEN;

use crate::pool_growth::{measure_growth, measure_raise_while_saturated};

use crate::worker_context::{observe_on_worker, observe_on_worker_while_impersonating};

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

// -- worker context (Probe E) --------------------------------------------

#[test]
fn a_thread_pool_worker_starts_with_no_impersonation_token() {
    let observed = observe_on_worker();

    assert!(
        observed.is_unimpersonated(),
        "a worker must start with no token, got {observed:?}"
    );
    assert_eq!(
        observed.open_token_error, ERROR_NO_TOKEN,
        "and the reason must be ERROR_NO_TOKEN rather than a failure to ask"
    );
}

#[test]
fn a_worker_does_not_inherit_an_impersonating_submitters_token() {
    // The measurement the whole ambient-state crate rests on. Without it,
    // explicit capture would be unnecessary.
    let observed = observe_on_worker_while_impersonating();

    assert!(
        observed.is_unimpersonated(),
        "a worker must not inherit the submitter's token, got {observed:?}"
    );
}

#[test]
fn a_worker_starts_with_the_critical_error_handler_enabled() {
    // SEM_FAILCRITICALERRORS *suppresses* the handler, so its absence is what
    // leaves a modal dialog reachable from a shared pool thread.
    let observed = observe_on_worker();

    assert!(
        observed.critical_error_handler_enabled(),
        "a worker's error mode must leave the handler enabled, got {observed:?}"
    );
    assert_eq!(
        observed.error_mode & bits::FAIL_CRITICAL_ERRORS,
        0,
        "stated the other way round, so a reader cannot mistake the polarity"
    );
}

#[test]
fn the_submitting_thread_and_its_worker_disagree_about_identity() {
    // The control that gives the inheritance test its meaning: if the submitter
    // were not actually impersonating, "the worker has no token" would prove
    // nothing at all.
    let submitter_had_token = {
        let observed = observe_on_worker_while_impersonating();
        // observe_on_worker_while_impersonating asserts internally that the
        // submitter genuinely held a token before submitting; this records the
        // resulting asymmetry as the finding.
        observed.is_unimpersonated()
    };

    assert!(
        submitter_had_token,
        "the submitter impersonated and the worker did not inherit it"
    );
}

// -- pool growth (Probe P) -----------------------------------------------
//
// The ignored tier. Correct to assert, but these park real threads and wait on
// real clocks, so they are run deliberately -- notably on a new architecture,
// where the growth timing is exactly what might differ.

#[test]
#[ignore = "parks real threads and waits on real clocks (~1s); run deliberately"]
fn a_blocked_pool_grows_to_its_maximum() {
    // The assumption the saturation-response design rests on, and which the
    // pool API offers no getter for.
    let observed = measure_growth(4, 8, false);

    assert!(
        observed.saturated(),
        "the pool must reach its maximum while callbacks are blocked: {observed:?}"
    );
    assert!(
        observed.one_thread_each(),
        "blocked callbacks cannot share a thread: {observed:?}"
    );
}

#[test]
#[ignore = "parks real threads and waits on real clocks (~1s); run deliberately"]
fn the_pool_does_not_exceed_its_maximum() {
    // The control: growth that ignored the maximum would make the maximum
    // useless as a bound, and would mean this probe measures something else.
    let observed = measure_growth(2, 8, false);

    assert!(
        observed.started_while_blocked <= observed.maximum as usize,
        "no more than the maximum may run concurrently: {observed:?}"
    );
}

#[test]
#[ignore = "parks real threads and waits on real clocks (~1s); run deliberately"]
fn the_pool_grows_promptly_enough_for_a_short_stall_threshold() {
    // "Promptly" is what feeds a stall threshold, so it is measured rather than
    // assumed. A second is generous: the point is to catch a pool that takes
    // many seconds, which would make a short threshold meaningless.
    let observed = measure_growth(4, 8, false);

    assert!(
        observed.slowest_arrival() < Duration::from_secs(1),
        "the slowest worker arrived after {:?}: {observed:?}",
        observed.slowest_arrival()
    );
}

#[test]
#[ignore = "parks real threads and waits on real clocks (~2s); run deliberately"]
fn pool_growth_throttles_after_an_initial_burst() {
    // The most useful thing this probe found, and the one a design would most
    // easily get wrong: growth is NOT uniform. An initial burst of workers
    // arrives essentially immediately, and beyond that the pool adds roughly
    // one thread per throttle interval. Sizing a stall threshold from the burst
    // would be badly wrong about the tail.
    //
    // The burst size and interval are host-specific, so the *shape* is asserted
    // rather than the numbers; `probe-pool-growth` prints the numbers.
    let observed = measure_growth(8, 16, false);

    assert!(
        observed.saturated(),
        "the run must reach its maximum or it measures nothing: {observed:?}"
    );

    let burst = observed
        .throttles_after(Duration::from_millis(20))
        .expect("growth must visibly throttle at this size, or the finding has changed");

    assert!(
        burst >= 2,
        "an initial burst of at least two workers is expected, got {burst}: {observed:?}"
    );
    assert!(
        burst < observed.started_while_blocked,
        "and it must not account for every worker, or nothing throttled: {observed:?}"
    );
    assert!(
        observed.largest_gap() > Duration::from_millis(20),
        "the throttled regime must be visibly slower than the burst: {observed:?}"
    );
}

#[test]
#[ignore = "parks real threads and waits on real clocks (~2s); run deliberately"]
fn runs_long_also_reaches_the_maximum() {
    // SetThreadpoolCallbackRunsLong is documented to make the pool create
    // threads more eagerly when callbacks block, so the two settings are
    // compared rather than one being assumed to imply the other.
    let eager = measure_growth(4, 8, true);

    assert!(
        eager.saturated(),
        "runs-long must still reach the maximum: {eager:?}"
    );
}

#[test]
#[ignore = "parks real threads and waits on real clocks (~2s); run deliberately"]
fn raising_the_maximum_while_saturated_releases_more_work() {
    // The exact mechanism of "raise the pool size to compensate for a blocked
    // worker", so the delay is the number that matters.
    let delay = measure_raise_while_saturated(2, 6, 8);

    assert!(
        delay < Duration::from_secs(1),
        "raising the maximum took {delay:?} to take effect"
    );
}
