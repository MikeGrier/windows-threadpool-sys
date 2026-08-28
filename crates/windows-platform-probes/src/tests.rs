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

use crate::completion_port::measure as measure_completion_port;
use crate::device_map::{free_drive_letter, measure_with_subst};
use crate::ioring::{measure_registration, measure_thread_agnosticism};
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
        observed.worker.is_unimpersonated(),
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
    let observed = observe_on_worker_while_impersonating();

    // Both sides, from the same run. Reading the worker's state twice -- which
    // an earlier version of this test did, by binding `submitter_had_token` to
    // the worker's `is_unimpersonated()` -- asserts the inheritance finding a
    // second time and checks no asymmetry at all.
    assert!(
        observed.submitter.has_thread_token,
        "the submitting thread must hold a token, or there is no asymmetry to \
         observe: {observed:?}"
    );
    assert!(
        observed.worker.is_unimpersonated(),
        "the worker must hold none: {observed:?}"
    );
    assert!(
        observed.disagree(),
        "the submitter impersonated and the worker did not inherit it: {observed:?}"
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
    let observed = measure_raise_while_saturated(2, 6, 8);

    // The premise, before the number that depends on it. The probe's settle
    // loop exits on saturation *or* timeout, so without this a pool that had
    // reached only 1 of its 2 base threads would have the delay below time
    // ordinary growth toward the base maximum -- a small number, passing the
    // assertion, measuring the wrong thing entirely.
    assert!(
        observed.saturated_before_raise(),
        "the pool must be saturated at its base maximum before the raise, or \
         the delay measures growth toward that base rather than the raise: \
         {observed:?}"
    );
    assert!(
        observed.took_effect,
        "no extra callback started within the settle window, so the delay is \
         the window itself and not a measurement: {observed:?}"
    );
    assert!(
        observed.delay < Duration::from_secs(1),
        "raising the maximum took {:?} to take effect: {observed:?}",
        observed.delay
    );
}

// -- device map (Probe DM) -----------------------------------------------

#[test]
#[ignore = "defines and removes a drive letter, which is process-visible state; run deliberately"]
fn impersonation_changes_which_device_map_a_drive_letter_resolves_in() {
    // The measurement behind the session-relative drive-letter hazard that
    // windows-namespace-request-sys documents and deliberately does not close.
    let Some(letter) = free_drive_letter() else {
        panic!("no free drive letter on this host, so the probe cannot run");
    };

    let finding = measure_with_subst(&letter, r"\Device\HarddiskVolume1");

    assert!(
        finding.sessions_differ(),
        "the control must hold first: the two contexts must really be \
         different logon sessions, or a disappearing letter proves nothing. {finding:?}"
    );
    assert!(
        finding.impersonation_changes_the_map(),
        "the same letter on the same thread must resolve differently under a \
         different token: {finding:?}"
    );
}

#[test]
#[ignore = "defines and removes a drive letter, which is process-visible state; run deliberately"]
fn the_subst_letter_really_was_visible_before_impersonating() {
    // The fixture check. A letter that never resolved in our own session would
    // make the whole finding vacuous -- "not found while impersonating" would
    // be true of any letter at all.
    let Some(letter) = free_drive_letter() else {
        panic!("no free drive letter on this host, so the probe cannot run");
    };

    let finding = measure_with_subst(&letter, r"\Device\HarddiskVolume1");

    assert!(
        finding.own_session.is_found(),
        "the subst drive must exist in our own map, or the probe measured nothing: {finding:?}"
    );
    assert_eq!(
        finding.own_session.target.as_deref(),
        Some(r"\Device\HarddiskVolume1"),
        "and it must point where we put it: {finding:?}"
    );
}

// -- IoRing (Probes A, A2, B) --------------------------------------------

#[test]
#[ignore = "needs a recent Windows build with IoRing; environment-dependent"]
fn ioring_registration_replaces_the_table_rather_than_appending() {
    // windows-ioring-sys asserts this and refuses a second registration on the
    // strength of it, with the assertion recorded as explicitly UNVERIFIED.
    // This is the verification -- and it deliberately calls Win32 directly,
    // because probing through that crate's guard would confirm our own belief
    // by consulting it.
    let Some(observed) = measure_registration().measured() else {
        // A host without a ring cannot answer, which is not the same as the
        // answer being no.
        return;
    };

    assert!(
        observed.index_zero_usable_after_second,
        "the control must hold: index 0 is valid under either semantics, so its \
         failure means the probe broke rather than the table shrank. {observed:?}"
    );
    assert!(
        observed.replaces(),
        "registration must replace the whole table, or windows-ioring-sys's index \
         bookkeeping is wrong and its refusal a needless restriction. {observed:?}"
    );
    assert!(!observed.appends(), "and it must not append: {observed:?}");
}

#[test]
#[ignore = "needs a recent Windows build with IoRing; environment-dependent"]
fn an_ioring_operation_outlives_the_thread_that_submitted_it() {
    // Every thread in the proposed design is transient by construction, so a
    // thread-bound IRP would fail only under load.
    let Some(observed) = measure_thread_agnosticism().measured() else {
        return;
    };

    // The premise, checked rather than assumed: the read must still have been
    // outstanding when its submitter ended. An operation that had already
    // completed would be collected afterwards however thread-affine the
    // platform were, so a run without this establishes nothing. The probe uses
    // a pipe with nothing written to it precisely so this cannot be false.
    assert!(
        observed.pending_at_submitter_exit,
        "the read must still be outstanding at submitter exit, or the probe \
         measures nothing: {observed:?}"
    );
    assert!(
        observed.survives_submitter_exit(),
        "the operation must complete, and transfer real bytes, after its \
         submitter exited: {observed:?}"
    );
}

// -- completion-port fork (Probe D) --------------------------------------

#[test]
#[ignore = "needs a recent Windows build with IoRing; environment-dependent"]
fn iocp_association_forecloses_ioring_use_of_the_same_handle() {
    // The evidence for windows-namespace-request-sys returning an opened handle
    // plain and unassociated: the association is irreversible, so making it on
    // a caller's behalf silently removes a capability.
    let Some(finding) = measure_completion_port().measured() else {
        return;
    };

    assert!(
        finding.is_valid(),
        "the controls must hold first, or the probe is broken rather than the \
         platform answering: {finding:?}"
    );
    assert!(
        finding.association_forecloses_ioring(),
        "association must foreclose the ring path: {finding:?}"
    );
}

#[test]
#[ignore = "needs a recent Windows build with IoRing; environment-dependent"]
fn the_associated_handle_is_still_healthy_through_its_port() {
    // The control that makes the finding precise. Without it, "the ring read
    // failed" could mean the handle was broken outright rather than the ring
    // path specifically being refused -- a materially different claim.
    let Some(finding) = measure_completion_port().measured() else {
        return;
    };

    assert!(
        finding.port_still_works,
        "the associated handle must still complete through the port: {finding:?}"
    );
}

#[test]
#[ignore = "needs a recent Windows build with IoRing; environment-dependent"]
fn a_failed_ring_read_is_judged_on_more_than_where_the_completion_arrived() {
    // The regression this probe exists because of. The first version declared
    // COEXIST on seeing a completion arrive on the ring, while its result code
    // was ERROR_INVALID_PARAMETER and its byte count zero -- it checked *where*
    // the completion landed rather than *whether the operation succeeded*.
    //
    // This pins that a refused read is refused on every field, so the same
    // mistake cannot be made again without failing here.
    let Some(finding) = measure_completion_port().measured() else {
        return;
    };

    let refused = finding.after_iocp_association;
    assert!(
        !refused.succeeded(),
        "the read must be refused: {refused:?}"
    );
    assert!(
        refused.result_code < 0,
        "and refused by its result code, not merely by its payload: {refused:?}"
    );
    assert_eq!(refused.bytes, 0, "with no bytes transferred: {refused:?}");
    assert_eq!(
        refused.first_byte, 0,
        "and nothing landed in the buffer -- which a zero-filled fixture could \
         not have distinguished, hence the non-zero fill byte: {refused:?}"
    );
}

#[test]
#[ignore = "needs a recent Windows build with IoRing; environment-dependent"]
fn create_threadpool_io_forecloses_ioring_the_same_way() {
    // Measured rather than assumed to follow from the raw-IOCP case, because
    // CreateThreadpoolIo is the path this workspace actually uses -- so the
    // consequence lands on windows-threadpool-sys's own users.
    let Some(finding) = measure_completion_port().measured() else {
        return;
    };

    assert!(
        finding.before_threadpool_io.succeeded(),
        "the before-case must pass, or the after-case proves nothing: {finding:?}"
    );
    assert!(
        finding.threadpool_io_forecloses_ioring(),
        "CreateThreadpoolIo must foreclose the ring path too: {finding:?}"
    );
}
