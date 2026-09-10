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
use windows_sys::Win32::System::Diagnostics::Debug::SetThreadErrorMode;

use crate::device_map::{SubstDrive, measure_with_subst};
use crate::ioring::{measure_registration, measure_thread_agnosticism};
use crate::pool_growth::{measure_growth, measure_raise_while_saturated};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use crate::worker_context::{
    Impersonation, observe_on_worker, observe_on_worker_while_impersonating,
};

use crate::handle_state::{
    CursorObservation, Fixture, SingleShot, closing_duplicate_preserves_source,
    duplicate_shares_cursor, ground_truth, query_disturbs_cursor, separate_opens_are_independent,
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
fn the_invalid_bit_finding_survives_a_thread_that_already_carries_a_valid_bit() {
    // The probe reads its answer out of the mode left behind by a call that is
    // *expected to fail*, so the read-back is whatever the thread was already
    // carrying. Pre-load one of the valid bits: without the forced baseline
    // inside the probe, that bit shows up in the read-back and the probe
    // reports that the combined call installed it -- the exact opposite of what
    // happened, and a finding this crate would then record as fact.
    let mut previous = 0u32;
    // SAFETY: `previous` is a valid writable destination.
    let set = unsafe { SetThreadErrorMode(bits::FAIL_CRITICAL_ERRORS, &raw mut previous) };
    assert_ne!(set, 0, "pre-load a valid bit for the duration of this test");

    let (installed_nothing, read_back) = combined_invalid_installs_nothing();

    let mut ignored = 0u32;
    // SAFETY: restoring the exact value saved above.
    let restored = unsafe { SetThreadErrorMode(previous, &raw mut ignored) };
    assert_ne!(restored, 0, "restore this test's own thread error mode");

    assert!(
        installed_nothing,
        "the finding must not depend on what the thread already carried, but \
         the probe reported the valid bits installed: read back 0x{read_back:04X}"
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

    // ground_truth panics if the whole directory fits in one call, which is
    // the fixture-adequacy guard the sibling cursor tests already rely on.
    // This test needs it just as much: `restarted` compares the second
    // handle's names against the first's, so a single draining call would make
    // both the complete listing and the assertion below trivially true. It is
    // non-degenerate today only by coincidence of BUFFER_BYTES and
    // FIXTURE_FILES, which is exactly the sort of thing a later tidy-up
    // changes.
    let truth = ground_truth(&fixture);
    let observation = separate_opens_are_independent(&fixture);

    assert!(
        observation.left_mid_directory(&truth),
        "the first handle must be left part-way through, or there is no cursor \
         to have its own: {observation:?}"
    );
    assert!(
        observation.restarted(),
        "a separate open should have its own cursor: {observation:?}"
    );
}

#[test]
fn a_drained_first_call_does_not_count_as_a_left_cursor() {
    // The degenerate case the guard exists to reject, exercised directly
    // rather than hoped against: one call returned the whole directory, so
    // both handles hold the complete listing. `restarted` is satisfied -- and
    // means nothing, because no cursor was ever suspended.
    let truth = vec!["a".to_string(), "b".to_string()];
    let observation = CursorObservation {
        source_first: truth.clone(),
        other_next: Ok(truth.clone()),
    };

    assert!(
        observation.restarted(),
        "the degenerate case does satisfy restarted, which is the problem"
    );
    assert!(
        !observation.left_mid_directory(&truth),
        "so the guard must reject it: {observation:?}"
    );
}

#[test]
fn a_partial_first_call_counts_as_a_left_cursor() {
    // The real case: the first handle stopped part-way, and the second
    // returned the same opening names rather than continuing.
    let truth = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let observation = CursorObservation {
        source_first: vec!["a".to_string()],
        other_next: Ok(vec!["a".to_string()]),
    };

    assert!(
        observation.left_mid_directory(&truth),
        "a first call that stopped part-way did leave a cursor: {observation:?}"
    );
    assert!(observation.restarted(), "and the second handle restarted");
}

#[test]
fn an_empty_first_call_does_not_count_as_a_left_cursor() {
    // Nothing was read, so there is no cursor position to have kept or lost.
    let truth = vec!["a".to_string(), "b".to_string()];
    let observation = CursorObservation {
        source_first: Vec::new(),
        other_next: Ok(Vec::new()),
    };

    assert!(
        !observation.left_mid_directory(&truth),
        "an empty first call leaves no cursor: {observation:?}"
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
    let Some(drive) = SubstDrive::claim("map-differs") else {
        panic!("no free drive letter on this host, so the probe cannot run");
    };

    let finding = measure_with_subst(&drive);

    assert!(
        finding.claim_is_exclusive(),
        "the letter must carry only our own definition, or a sibling probe's \
         removal could be what the impersonated query is observing: {finding:?}"
    );
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
    let Some(drive) = SubstDrive::claim("visible-before") else {
        panic!("no free drive letter on this host, so the probe cannot run");
    };

    let finding = measure_with_subst(&drive);

    assert!(
        finding.own_session.is_found(),
        "the subst drive must exist in our own map, or the probe measured nothing: {finding:?}"
    );
    assert!(
        finding.claim_is_exclusive(),
        "and it must point where we put it, and nowhere else: {finding:?}"
    );
    assert_eq!(
        finding.own_session.entries(),
        [drive.target()],
        "stated as the exact entry list, so a stacked definition fails here \
         rather than being trimmed away silently: {finding:?}"
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

#[test]
fn the_impersonation_guard_reverts_even_while_unwinding() {
    // Pins the guard rather than the function that uses it, so the property is
    // checked without sabotaging the production path to reach a panic.
    //
    // What this protects: `observe_on_worker_while_impersonating` panics on two
    // reachable paths after impersonating -- the submitter-token assert, and
    // `observe_on_worker` itself when the pool refuses the callback or the
    // worker never reports. Before this guard those unwound past a plain
    // `RevertToSelf`, leaving the thread impersonating. Under
    // `--test-threads=1` libtest runs tests inline on the calling thread, so
    // every later test would inherit the identity and fail its own
    // precondition, burying the original failure.
    let outcome = std::panic::catch_unwind(|| {
        let _guard = Impersonation::apply();
        panic!("unwinding while the thread impersonates");
    });

    assert!(outcome.is_err(), "the panic must still propagate");

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentThread is a pseudo-handle needing no cleanup and
    // `token` is writable.
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &raw mut token) };
    if opened != 0 {
        // SAFETY: opened successfully, so this owns the handle.
        unsafe { CloseHandle(token) };
    }

    assert_eq!(
        opened, 0,
        "the thread must not have been left impersonating after the unwind"
    );
}

// --- topology -------------------------------------------------------------
//
// Every interesting number the topology probe prints is host-specific, so
// nothing here asserts a *value*. What is asserted is internal consistency,
// which must hold on any machine and therefore catches a parsing regression in
// `windows-topology-sys` on whatever hardware CI happens to run on -- which is
// the whole reason the probe reads the shipping crate rather than a second
// parse written here.

#[test]
fn the_machine_reports_at_least_one_processor_one_group_and_one_core() {
    let observation = crate::topology::measure().expect("topology discovery");

    assert!(
        observation.online_processors >= 1,
        "a running process implies at least one online processor"
    );
    assert!(
        observation.groups >= 1,
        "every machine has at least one processor group"
    );
    // Phrased as claims about the REPORT, not the machine. The machine
    // certainly has both; a zero here means the enumeration did not describe
    // them, and saying "every machine has at least one package" would send a
    // reader to doubt the hardware.
    assert!(
        !observation.cores.is_empty(),
        "no cores were reported, so the enumeration did not describe a machine that has them"
    );
    assert!(
        observation.packages >= 1,
        "no packages were reported, so the enumeration did not describe a machine that has one"
    );
}

#[test]
fn the_shipping_parse_agrees_with_the_raw_win32_counters() {
    let observation = crate::topology::measure().expect("topology discovery");

    // This is the cross-check that makes the probe worth running everywhere: a
    // disagreement means windows-topology-sys parsed
    // GetLogicalProcessorInformationEx differently from what the simple
    // counters report on this host.
    // The VERDICT, not just an empty disagreement list. Asserting only that
    // nothing disagreed would pass on a run where a counter could not be read
    // and so was never compared -- which is the state this probe must never
    // report as agreement.
    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Agree,
        // The whole `CrossCheck`, not two of its three lists. Naming the lists
        // individually meant `parse_incomplete` -- added later as a third way to
        // miss `Agree` -- was omitted, so the one host that would newly fail
        // here got "disagreements=[] not_compared=[]": an assertion saying
        // agreement was not established, beside evidence that nothing
        // disagreed and nothing was skipped. The reason was in `check` all
        // along and was discarded at the point of failure.
        "cross-check did not establish agreement: {check:?}"
    );
}

/// An observation with nothing to complain about, for a test to perturb one
/// field of. Every host reachable here has a single NUMA node, so the sparse
/// case below cannot be measured and has to be constructed.
///
/// It carries one whole-machine cache level rather than an empty list, and the
/// difference is load-bearing: an empty `caches` is now itself a finding (no
/// cache level was reported at all, so nothing establishes what divides this
/// machine), and a fixture that tripped it would make every test perturbing one
/// other field assert against two complaints instead of one. That this fixture
/// was previously empty and uncomplained-about is exactly the defect -- nine
/// tests turned red the moment the finding was added, which is the fixture
/// telling the truth about what it had been relying on.
fn agreeing_observation() -> crate::topology::Observation {
    crate::topology::Observation {
        online_processors: 4,
        groups: 1,
        numa_domains: 1,
        numa_domains_without_processors: 0,
        numa_domains_only_in_cpu_sets: 0,
        numa_domains_unreported: 0,
        numa_domains_with_conflicting_labels: 0,
        topology_was_measured: true,
        cores_only_in_cpu_sets: 0,
        cores_without_processors: 0,
        packages_without_processors: 0,
        overlapping_walk_relations: 0,
        described_relations: 0,
        unreported_relations: 0,
        processor_attribute_conflicts: 0,
        highest_numa_node: Some(0),
        packages: 1,
        // One core, for the same reason it carries one cache level: reporting
        // none is itself a finding now, and a fixture that tripped it would
        // make every test perturbing one other field assert against two
        // complaints instead of one.
        //
        // SMT is `true` because the core carries four processors, and the owning
        // crate defines the flag as exactly that condition. It read `false`
        // while `cross_check` did not look at the pair, so this fixture -- the
        // one every perturbation test starts from -- described a core that
        // cannot exist.
        cores: vec![crate::topology::CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 4,
        }],
        caches: vec![crate::topology::CacheLevel {
            level: 1,
            processors_per_domain: vec![4],
        }],
        partitioning_cache_level: None,
        enumeration_anomalies: Vec::new(),
        coherence: windows_topology_sys::Coherence::Agreed,
        raw_active_processors: 4,
        raw_group_count: 1,
        raw_highest_numa_node: Some(0),
        bracket: crate::topology::BracketOutcome::HeldStill,
    }
}

#[test]
fn sparse_numa_node_numbers_are_not_reported_as_a_parsing_regression() {
    // `GetNumaHighestNodeNumber` reports the highest node *number*, which
    // Windows does not promise equals the node count. Nodes 0 and 2 are a valid
    // sparse topology: two domains, highest number two. Comparing the count
    // against `highest + 1` called that a disagreement, so the probe's asserted
    // test would fail on hardware that is reporting itself correctly.
    let mut observation = agreeing_observation();
    observation.numa_domains = 2;
    observation.highest_numa_node = Some(2);
    observation.raw_highest_numa_node = Some(2);

    assert_eq!(
        observation.cross_check().verdict(),
        crate::topology::Verdict::Agree,
        "a sparse node numbering is a valid machine, not a parse error"
    );
}

#[test]
fn a_numa_node_the_topology_crate_never_saw_is_still_reported() {
    // The other direction, so the sparse tolerance cannot pass by never
    // complaining: Windows names a node the crate's parse did not produce, and
    // that is the disagreement this cross-check exists to surface.
    let mut observation = agreeing_observation();
    observation.raw_highest_numa_node = Some(3);

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Disagree,
        "{check:?}"
    );
    assert_eq!(check.disagreements.len(), 1, "{check:?}");
    assert!(check.disagreements[0].contains("NUMA nodes"), "{check:?}");
    assert!(check.not_compared.is_empty(), "{check:?}");
}

#[test]
fn a_topology_reporting_no_numa_node_at_all_disagrees_with_a_raw_one() {
    // The `None` arm, which the count form could not express: Windows names a
    // node and the crate's parse produced no memory domain whatsoever.
    let mut observation = agreeing_observation();
    observation.numa_domains = 0;
    observation.highest_numa_node = None;

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Disagree,
        "{check:?}"
    );
    assert_eq!(check.disagreements.len(), 1, "{check:?}");
    assert!(check.disagreements[0].contains("none"), "{check:?}");
}

// A counter that could not be read must never read as agreement. Each of these
// perturbs one counter into its documented failure report and asserts the
// verdict is `Incomplete` -- not `Agree`, which would claim something the run
// did not establish, and not `Disagree`, which would blame the shipping crate
// for a measurement this probe failed to take.

#[test]
fn a_failed_numa_read_is_incomplete_rather_than_agreement() {
    // The reachable one: the renderer already has a `failed` branch for this
    // counter, and the verdict beneath it used to say "agree ... parsed this
    // machine consistently" on the very next line.
    let mut observation = agreeing_observation();
    observation.raw_highest_numa_node = None;

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert_eq!(check.not_compared.len(), 1, "{check:?}");
    assert!(
        check.not_compared[0].contains("GetNumaHighestNodeNumber"),
        "{check:?}"
    );
}

#[test]
fn a_failed_processor_count_is_incomplete_rather_than_a_parse_disagreement() {
    // `GetActiveProcessorCount` reports failure by returning zero, and no
    // machine has zero active processors. Compared as a count, that zero
    // produced "topology crate says 4, GetActiveProcessorCount says 0" -- an
    // accusation against the crate's parse for a read this probe fumbled.
    let mut observation = agreeing_observation();
    observation.raw_active_processors = 0;

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(
        check.not_compared[0].contains("GetActiveProcessorCount"),
        "{check:?}"
    );
}

#[test]
fn a_failed_group_count_is_incomplete_rather_than_a_parse_disagreement() {
    let mut observation = agreeing_observation();
    observation.raw_group_count = 0;

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(
        check.not_compared[0].contains("GetActiveProcessorGroupCount"),
        "{check:?}"
    );
}

#[test]
fn a_dropped_enumeration_record_blocks_agreement_even_when_every_counter_matches() {
    // The counters are three scalars, and none of them is sensitive to a
    // `PROCESSOR_RELATIONSHIP` record that failed to decode: `discover` records
    // the anomaly, drops the record, and returns `Ok`, so the package and core
    // counts are short by an amount nothing here can observe. All three
    // counters therefore still agree, and the probe printed "agree ... parsed
    // this machine consistently" for a parse the crate had already reported as
    // incomplete.
    //
    // The fixture is otherwise the agreeing one on purpose: the anomaly is the
    // only difference, so this cannot pass by way of some other complaint.
    let mut observation = agreeing_observation();
    observation.enumeration_anomalies = vec![windows_topology_sys::EnumerationAnomaly {
        source: windows_topology_sys::Source::RelationshipWalk,
        offset: 64,
        kind: windows_topology_sys::AnomalyKind::Undersized {
            declared: 8,
            minimum: 48,
        },
    }];

    let check = observation.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "an undecodable record is not the crate parsing something wrongly: {check:?}"
    );
    assert!(
        check.not_compared.is_empty(),
        "every counter was read, so nothing belongs in not_compared: {check:?}"
    );
    assert_eq!(check.parse_incomplete.len(), 1, "{check:?}");
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "agreeing counters must not certify a parse the crate said was short: {check:?}"
    );
}

#[test]
fn disagreeing_source_enumerations_block_agreement_even_when_every_counter_matches() {
    // `Coherence::Disagreed` is the crate's *conclusion*, after exhausting its
    // retries, that its two Win32 sources describe different machines -- and it
    // returns `Ok`. The processors in `cpu_sets_only` are deliberately absent
    // from the parsed processor list, so no count taken from that list can
    // reveal them and `GetActiveProcessorCount` need not notice: it counts
    // ACTIVE processors, while the parsed list carries inactive slots too, so
    // the two totals can coincide while the membership differs.
    let mut observation = agreeing_observation();
    observation.coherence = windows_topology_sys::Coherence::Disagreed {
        walk_only: Vec::new(),
        cpu_sets_only: vec![windows_topology_sys::ProcessorId {
            group: 0,
            number: 3,
        }],
        attempts: 4,
    };

    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(check.not_compared.is_empty(), "{check:?}");
    assert_eq!(check.parse_incomplete.len(), 1, "{check:?}");
    assert!(
        check.parse_incomplete[0].contains("never agreed"),
        "{check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
}

#[test]
fn a_numa_domain_only_cpu_sets_reported_blocks_agreement_rather_than_accusing_the_parse() {
    // `fold_memberships` pushes a memory domain that only CPU Sets described --
    // "the walk not describing it is a fact about the walk, not evidence the
    // relation is not there". Neither `coherence` nor any counter reaches it:
    // coherence compares PROCESSOR SETS, and two sources can name the same
    // processors while grouping them into different nodes.
    let mut observation = agreeing_observation();
    observation.numa_domains = 2;
    observation.numa_domains_only_in_cpu_sets = 1;

    let check = observation.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "the sources grouping nodes differently is not the crate contradicting a counter: {check:?}"
    );
    assert_eq!(check.parse_incomplete.len(), 1, "{check:?}");
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
}

#[test]
fn observe_takes_the_node_number_from_whichever_source_reported_it() {
    // Drives the extraction itself rather than a hand-set field, which is why
    // `observe` was split out of `measure`: the defect lived in this loop and
    // sat behind `MachineMemoryTopology::discover`, so no test could reach it.
    //
    // Two memory domains. Node 0 is what the relationship walk saw; node 1 is a
    // relation only CPU Sets reported, which `fold_memberships` pushes as its
    // own domain. Reading the walk's label alone made node 1 raise
    // `numa_domains` to 2 while leaving the highest at 0 -- so a machine whose
    // GetNumaHighestNodeNumber says 1 was accused of a parsing regression for a
    // node the crate had reported correctly.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Source,
    };

    let memory = || DomainKind::Memory {
        memory_bytes: windows_topology_sys::Observed::NotObserved,
    };
    let processor = |number| Processor {
        id: ProcessorId { group: 0, number },
        online: true,
        capacity: 0,
    };
    // Two online processors in one group, so the counters passed below match
    // and the ONLY thing this fixture can complain about is the node number.
    let topology = MachineMemoryTopology {
        processors: vec![processor(0), processor(1)],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8), (0u16, 1u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: memory(),
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: memory(),
                processors: [(0u16, 1u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::CpuSets, 1)],
            },
        ],
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        2,
        1,
        Some(1),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.numa_domains, 2);
    assert_eq!(
        observation.highest_numa_node,
        Some(1),
        "node 1 was reported by CPU Sets, and node numbers are machine-wide from \
         either source, so the highest the crate reported is 1"
    );
    assert_eq!(
        observation.numa_domains_only_in_cpu_sets, 1,
        "and the domain the walk never described is counted, because nothing else sees it"
    );

    let check = observation.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "the counter says 1 and so does the crate: {check:?}"
    );
}

#[test]
fn no_partitioning_level_is_told_apart_from_no_unique_one() {
    use crate::topology::{CacheLevel, PartitioningCache};

    // The distinction the NDJSON used to collapse. Both render a level of
    // `null`, and both reach `Verdict::Agree` because neither touches anything
    // `cross_check` consults -- so a fleet query counting nulls as "machines no
    // cache level partitions" silently folded in machines where a level DOES
    // partition, which is the opposite conclusion for anything sizing itself by
    // cache boundary.
    let mut nothing_partitions = agreeing_observation();
    nothing_partitions.caches = vec![CacheLevel {
        level: 3,
        processors_per_domain: vec![4],
    }];
    nothing_partitions.partitioning_cache_level = None;
    assert_eq!(
        nothing_partitions.partitioning_cache(),
        PartitioningCache::NoLevelPartitions,
        "one level covering the whole machine cannot have partitioned it"
    );

    // Same `None` from the crate, but a level plainly splits the machine. The
    // crate reaches that for two incomparable maximal candidates AND for a
    // candidate filter that left nothing, so this is named as "not the case
    // above" and no more.
    let mut something_partitions = agreeing_observation();
    something_partitions.caches = vec![CacheLevel {
        level: 3,
        processors_per_domain: vec![2, 2],
    }];
    something_partitions.partitioning_cache_level = None;
    assert_eq!(
        something_partitions.partitioning_cache(),
        PartitioningCache::NoUniqueOutermost,
        "a level with two domains partitions it, whatever the crate could name"
    );

    // Both still agree, which is the point: the verdict was never going to
    // carry this distinction, so the JSON had to.
    assert_eq!(
        nothing_partitions.cross_check().verdict(),
        crate::topology::Verdict::Agree
    );
    assert_eq!(
        something_partitions.cross_check().verdict(),
        crate::topology::Verdict::Agree
    );
}

#[test]
fn a_cache_level_that_decoded_to_no_partitions_blocks_the_no_partitioning_conclusion() {
    use crate::topology::{CacheLevel, PartitioningCache};

    // `NoLevelPartitions` is read as a fact about the hardware -- no cache
    // boundary divides the work -- and a level with ZERO partitions satisfies
    // "not more than one" exactly like a level covering the whole machine. It
    // is not the same finding: that level's records decoded to nothing.
    //
    // Nothing else here is sensitive to it. A well-formed GROUP_AFFINITY with a
    // zero mask decodes completely and raises no anomaly, so without this the
    // report printed "L3 0 domain(s)" a few lines above "no cache level
    // partitions this machine", with the verdict certifying the pair as agree.
    let mut observation = agreeing_observation();
    observation.caches = vec![CacheLevel {
        level: 3,
        processors_per_domain: Vec::new(),
    }];
    observation.partitioning_cache_level = None;

    assert_eq!(
        observation.partitioning_cache(),
        PartitioningCache::NoLevelPartitions,
        "the classification is unchanged; what changes is that it is caveated"
    );

    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert_eq!(check.parse_incomplete.len(), 1, "{check:?}");
    assert!(
        check.parse_in_doubt(),
        "so the renderer caveats it: {check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "and a mining pass filtering on agree never sees the row: {check:?}"
    );
}

#[test]
fn no_cache_levels_at_all_is_its_own_answer_and_is_caveated() {
    use crate::topology::PartitioningCache;

    // One step past the zero-partition level. `any()` over an EMPTY list is
    // vacuously false, so an empty survey reached `NoLevelPartitions` and
    // printed "no cache boundary divides the work" -- a conclusion about cache
    // structure from a survey that found no cache structure. Nothing else here
    // notices: no record failed to decode, because there was no record.
    //
    // Reachable on exactly the fleet this probe surveys -- a hypervisor
    // reporting group, core and NUMA relationships but no cache ones.
    let mut observation = agreeing_observation();
    observation.caches = Vec::new();
    observation.partitioning_cache_level = None;

    assert_eq!(
        observation.partitioning_cache(),
        PartitioningCache::NoLevelsReported,
        "an empty survey is not the same finding as a survey that found no partitions"
    );

    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert_eq!(check.parse_incomplete.len(), 1, "{check:?}");
    assert!(check.parse_in_doubt(), "{check:?}");
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "so a mining pass filtering on agree never sees the row: {check:?}"
    );
}

#[test]
fn a_level_that_covers_the_whole_machine_is_not_caveated() {
    use crate::topology::CacheLevel;

    // The control. A genuine single-domain level is the ordinary "no cache
    // boundary divides the work" machine and must stay `Agree`, or the caveat
    // above would fire on every uniform host and mean nothing.
    let mut observation = agreeing_observation();
    observation.caches = vec![CacheLevel {
        level: 3,
        processors_per_domain: vec![4],
    }];
    observation.partitioning_cache_level = None;

    let check = observation.cross_check();
    assert!(!check.parse_in_doubt(), "{check:?}");
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Agree,
        "{check:?}"
    );
}

#[test]
fn partitioning_cache_returns_the_summary_for_the_named_level_among_several() {
    use crate::topology::{CacheLevel, PartitioningCache};

    // Several levels, so the lookup has something to get WRONG. With one entry
    // a mutated `==` finds nothing and yields `SummaryMissing`, which no test
    // distinguished from the right answer; with three it returns a different
    // level's summary, which this catches.
    let mut observation = agreeing_observation();
    observation.caches = vec![
        CacheLevel {
            level: 1,
            processors_per_domain: vec![1, 1, 1, 1],
        },
        CacheLevel {
            level: 2,
            processors_per_domain: vec![2, 2],
        },
        CacheLevel {
            level: 3,
            processors_per_domain: vec![4],
        },
    ];
    observation.partitioning_cache_level = Some(2);

    match observation.partitioning_cache() {
        PartitioningCache::Level(cache) => {
            assert_eq!(
                cache.level, 2,
                "the summary must be the one the crate named"
            );
            assert_eq!(cache.domains(), 2);
        }
        other => panic!("expected the named level's summary, got {other:?}"),
    }
}

#[test]
fn observe_counts_a_numa_domain_with_no_processors() {
    // Drives the extraction. The consequence was pinned by a hand-set field, so
    // a mutation sweep found the increment itself replaceable.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Observed, Processor, ProcessorId,
        Source,
    };

    let memory = || DomainKind::Memory {
        memory_bytes: Observed::NotObserved,
    };
    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: memory(),
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            // The CXL-expander shape: memory with no processors at all.
            Domain {
                kind: memory(),
                processors: windows_topology_sys::ProcessorSet::default(),
                observations: vec![Observation::new(Source::RelationshipWalk, 1)],
            },
        ],
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        Some(1),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.numa_domains, 2);
    assert_eq!(
        observation.numa_domains_without_processors, 1,
        "and it is subtracted from the by-numa-domain policy, which is why it is counted"
    );
    assert_eq!(
        observation
            .domain_counts()
            .into_iter()
            .find(|(name, _)| *name == "by-numa-domain-with-processors")
            .map(|(_, count)| count),
        Some(1),
    );
}

#[test]
fn a_topology_with_no_processors_at_all_is_not_accused_of_hiding_packages() {
    // The boundary on the `online_processors > 0` guards. A synthetic topology
    // that reported nothing has no processors either, so "no packages were
    // reported, though the machine has one" would assert a machine this
    // observation never saw.
    let mut observation = agreeing_observation();
    observation.online_processors = 0;
    observation.raw_active_processors = 0;
    observation.packages = 0;
    observation.cores = Vec::new();

    let check = observation.cross_check();
    assert!(
        !check.parse_incomplete.iter().any(
            |c| c.contains("no packages were reported") || c.contains("no cores were reported")
        ),
        "with no processors reported, absent packages and cores are not a separate finding: \
         {check:?}"
    );
}

#[test]
fn a_named_level_with_no_summary_is_a_probe_defect_not_a_fact_about_the_machine() {
    use crate::topology::PartitioningCache;

    // Asserted unreachable by
    // `the_outermost_partitioning_cache_is_the_deepest_level_that_splits_the_machine`.
    // Pinned as its own variant anyway, because folding it into the `None` arm
    // is what would print "no cache level partitions this machine" -- a claim
    // about the hardware -- when the truth is that this probe lost the summary.
    let mut observation = agreeing_observation();
    observation.caches = Vec::new();
    observation.partitioning_cache_level = Some(3);

    assert_eq!(
        observation.partitioning_cache(),
        PartitioningCache::SummaryMissing(3)
    );
}

#[test]
fn observe_reports_a_numa_domain_whose_sources_number_it_differently() {
    // Node numbers are machine-wide, so one domain cannot honestly be both node
    // 0 and node 1 -- the two sources contradict each other. The crate keeps
    // both labels on purpose ("the labels differ and both are kept, which is
    // the whole of D-15"); taking the maximum to compare against the counter
    // resolves the disagreement silently, which is what this stops.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Observed, Processor, ProcessorId,
        Source,
    };

    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Memory {
                    memory_bytes: Observed::NotObserved,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![
                    Observation::new(Source::RelationshipWalk, 0),
                    Observation::new(Source::CpuSets, 1),
                ],
            },
        ],
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        Some(1),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.numa_domains_with_conflicting_labels, 1);
    assert_eq!(
        observation.highest_numa_node,
        Some(1),
        "the maximum is still taken, so the counter comparison can proceed"
    );

    let check = observation.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "the sources contradicting each other is not the crate contradicting a counter: {check:?}"
    );
    // The entry, not the count. This fixture carries no cache domains either,
    // so the empty-survey entry fires alongside; asserting a total would couple
    // this test to causes it is not about.
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("more than one distinct node number")),
        "{check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "agreeing counters must not certify a node number the sources disputed: {check:?}"
    );
}

#[test]
fn a_machine_that_changed_mid_run_is_not_compared_rather_than_blamed() {
    // The parse and the counters are separate reads. A processor hot-add
    // between them leaves both correct for different instants, and comparing
    // them would file the difference as the crate parsing wrongly.
    let mut observation = agreeing_observation();
    observation.bracket = crate::topology::BracketOutcome::Changed;
    observation.groups = 1;
    observation.raw_group_count = 9;

    let check = observation.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "a machine that moved under the run is this measurement's problem, not the parse's: \
         {check:?}"
    );
    assert_eq!(check.not_compared.len(), 1, "{check:?}");
    assert!(
        !check.parse_in_doubt(),
        "the topology is still a valid snapshot of the machine as it was, so the cache \
         conclusions drawn from it stand: {check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
}

#[test]
fn a_machine_that_changed_still_reports_what_the_parse_itself_lost() {
    // The combined case, which the machine-changed test above cannot reach
    // because it starts from a clean fixture. A host can hot-add a processor
    // AND have dropped a record; the two are unrelated.
    //
    // Handling the timing skew with an early return suppressed every
    // parse-side check, so this reported `parse_incomplete: 0`,
    // `parse_in_doubt` was false, and the renderer printed hardware
    // conclusions uncaveated while never mentioning the dropped record
    // anywhere in the report.
    let mut observation = agreeing_observation();
    observation.bracket = crate::topology::BracketOutcome::Changed;
    observation.enumeration_anomalies = vec![windows_topology_sys::EnumerationAnomaly {
        source: windows_topology_sys::Source::RelationshipWalk,
        offset: 64,
        kind: windows_topology_sys::AnomalyKind::Undersized {
            declared: 8,
            minimum: 48,
        },
    }];

    let check = observation.cross_check();
    assert!(
        !check.not_compared.is_empty(),
        "the timing skew is still recorded: {check:?}"
    );
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("enumeration anomal")),
        "and the dropped record is NOT suppressed by it: {check:?}"
    );
    assert!(
        check.parse_in_doubt(),
        "so the renderer still caveats its hardware conclusions: {check:?}"
    );
}

#[test]
fn a_core_or_attribute_the_sources_disagree_about_blocks_agreement() {
    // The core-level and attribute-level twins of the NUMA provenance check.
    // `coherence` reaches neither: it compares which PROCESSORS exist, so two
    // sources can agree on every processor and still group them into different
    // cores, or give the same processor different efficiency classes.
    for (label, mutate) in [
        (
            "core(s) were reported only by CPU Sets",
            Box::new(|o: &mut crate::topology::Observation| o.cores_only_in_cpu_sets = 1)
                as Box<dyn Fn(&mut crate::topology::Observation)>,
        ),
        (
            "attribute(s) carry more than one distinct value",
            Box::new(|o: &mut crate::topology::Observation| o.processor_attribute_conflicts = 1),
        ),
    ] {
        let mut observation = agreeing_observation();
        mutate(&mut observation);

        let check = observation.cross_check();
        assert!(
            check.disagreements.is_empty(),
            "{label}: the sources disagreeing with each other is not the crate contradicting a \
             counter: {check:?}"
        );
        assert!(
            check.parse_incomplete.iter().any(|c| c.contains(label)),
            "{label}: {check:?}"
        );
        assert_eq!(
            check.verdict(),
            crate::topology::Verdict::Incomplete,
            "{label}: {check:?}"
        );
    }
}

#[test]
fn observe_counts_a_core_only_cpu_sets_reported() {
    // Drives the extraction, so the provenance check cannot be dropped
    // silently. The walk describes one core over both processors; CPU Sets
    // describes a different core over one of them, which `fold_memberships`
    // keeps as its own domain because the memberships differ.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Source,
    };

    let core = |smt: bool| DomainKind::Core {
        simultaneous_multithreading: smt,
        efficiency_class: 0,
    };
    let topology = MachineMemoryTopology {
        processors: (0..2)
            .map(|number| Processor {
                id: ProcessorId { group: 0, number },
                online: true,
                capacity: 0,
            })
            .collect(),
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8), (0u16, 1u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: core(true),
                processors: [(0u16, 0u8), (0u16, 1u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: core(false),
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::CpuSets, 0)],
            },
        ],
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        2,
        1,
        Some(0),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.cores.len(), 2, "both groupings are carried");
    assert_eq!(
        observation.cores_only_in_cpu_sets, 1,
        "and the one the walk never described is counted, so the inflated \
         by-core count cannot be read as an established core count"
    );
}

#[test]
fn observe_carries_the_crates_attribute_conflicts() {
    // Pins the WIRING, not just the consequence. The test above sets
    // `processor_attribute_conflicts` by hand, so replacing this call with a
    // constant zero left it green -- the guard was covered and the thing that
    // feeds the guard was not.
    //
    // Two sources give processor 0 a different efficiency class. There is no
    // membership to compare, which is why the crate keeps this apart from the
    // relations, and why nothing else in this observation can see it.
    use windows_topology_sys::{
        AttributeObservation, Domain, DomainKind, MachineMemoryTopology, Observation,
        ProcessorAttribute, ProcessorId, Source,
    };

    let processor = ProcessorId {
        group: 0,
        number: 0,
    };
    let topology = MachineMemoryTopology {
        processors: vec![windows_topology_sys::Processor {
            id: processor,
            online: true,
            capacity: 0,
        }],
        domains: vec![Domain {
            kind: DomainKind::Group,
            processors: [(0u16, 0u8)].into_iter().collect(),
            observations: vec![Observation::new(Source::RelationshipWalk, 0)],
        }],
        processor_attributes: vec![
            AttributeObservation::new(
                processor,
                ProcessorAttribute::EfficiencyClass,
                0,
                Source::RelationshipWalk,
            ),
            AttributeObservation::new(
                processor,
                ProcessorAttribute::EfficiencyClass,
                1,
                Source::CpuSets,
            ),
        ],
        ..MachineMemoryTopology::default()
    };

    assert_eq!(
        topology.attribute_conflicts().len(),
        1,
        "the fixture must actually produce a conflict, or this proves nothing"
    );

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        Some(0),
        crate::topology::BracketOutcome::HeldStill,
    );
    assert_eq!(observation.processor_attribute_conflicts, 1);
    assert!(
        observation
            .cross_check()
            .parse_incomplete
            .iter()
            .any(|c| c.contains("attribute(s) carry more than one distinct value")),
    );
}

#[test]
fn a_topology_nobody_measured_cannot_be_certified_against_this_machine() {
    // `observe` is public and takes any topology. `MachineMemoryTopology`
    // defaults to `Provenance::Synthetic` and deserialization downgrades to it,
    // precisely so a topology nobody measured cannot pass for one that was --
    // and without reading that, a hand-built or restored topology with matching
    // counters reached `Agree`, certifying consistency with a host no
    // enumeration ever read.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let build = |provenance| MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
        ],
        provenance,
        ..MachineMemoryTopology::default()
    };

    let synthetic = crate::topology::observe(
        &build(Provenance::Synthetic),
        1,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );
    assert!(!synthetic.topology_was_measured);
    assert!(
        synthetic
            .cross_check()
            .parse_incomplete
            .iter()
            .any(|c| c.contains("not measured from a running machine")),
        "{:?}",
        synthetic.cross_check()
    );

    // The control: the same topology, measured, carries no such entry -- so the
    // finding is about provenance and not about the fixture's shape.
    let measured = crate::topology::observe(
        &build(Provenance::Measured),
        1,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );
    assert!(measured.topology_was_measured);
    assert!(
        !measured
            .cross_check()
            .parse_incomplete
            .iter()
            .any(|c| c.contains("not measured from a running machine")),
        "{:?}",
        measured.cross_check()
    );
}

#[test]
fn a_counter_that_failed_one_read_is_not_reported_as_the_machine_changing() {
    // `before != after` treated the failure sentinels as values, so a counter
    // that failed once and succeeded once read as "the machine changed while
    // this ran" -- a claim about the hardware nothing established, which also
    // skipped all three comparisons.
    //
    // THE shipping predicate, not a copy of it. This test reproduced the
    // expression verbatim and asserted against its own closure, so a mutation
    // sweep found all six operators in `measure` unkilled -- the test was named
    // for a regression it could not detect.
    let changed = |before, after| {
        crate::topology::bracket_outcome(before, after) == crate::topology::BracketOutcome::Changed
    };

    assert!(
        !changed((0, 1, Some(0)), (4, 1, Some(0))),
        "a counter that failed the first read and succeeded the second says nothing about change"
    );
    assert!(
        !changed((4, 1, Some(0)), (0, 1, Some(0))),
        "nor the reverse"
    );
    assert!(
        !changed((4, 1, None), (4, 1, Some(0))),
        "nor the NUMA call, whose sentinel is None"
    );
    // The group clause too. A mutation sweep found all three of its operators
    // unkilled while the other two clauses were covered -- the test asserted
    // the rule and exercised it on two of the three counters.
    assert!(
        !changed((4, 0, Some(0)), (4, 1, Some(0))),
        "nor the group count, whose sentinel is also 0"
    );

    assert!(
        changed((4, 1, Some(0)), (8, 1, Some(0))),
        "two good reads that differ ARE evidence the machine changed"
    );
    assert!(
        changed((4, 1, Some(0)), (4, 2, Some(0))),
        "on the group count as well"
    );
    assert!(
        changed((4, 1, Some(0)), (4, 1, Some(1))),
        "and on the NUMA highest node"
    );
}

#[test]
fn a_report_of_no_packages_or_no_cores_is_a_finding_not_a_machine() {
    // The machine has both whatever the enumeration said. No record needs to
    // have FAILED for this to happen -- Windows can simply not report the
    // relationship, so no anomaly fires and nothing else here notices. Treated
    // exactly as an empty cache survey is.
    for (label, mutate) in [
        (
            "packages",
            Box::new(|o: &mut crate::topology::Observation| o.packages = 0)
                as Box<dyn Fn(&mut crate::topology::Observation)>,
        ),
        (
            "cores",
            Box::new(|o: &mut crate::topology::Observation| o.cores = Vec::new()),
        ),
    ] {
        let mut observation = agreeing_observation();
        mutate(&mut observation);

        let check = observation.cross_check();
        assert!(
            check.disagreements.is_empty(),
            "{label}: an absent relationship is not the crate contradicting a counter: {check:?}"
        );
        assert!(
            check.parse_incomplete.iter().any(|c| c.contains(label)),
            "{label}: {check:?}"
        );
        assert_eq!(
            check.verdict(),
            crate::topology::Verdict::Incomplete,
            "{label}: {check:?}"
        );
    }
}

#[test]
fn a_numa_domain_no_source_reported_blocks_agreement_rather_than_accusing_the_parse() {
    // A memory domain with an empty `observations` list is what
    // windows-topology-sys documents as the honest state for "a relation nobody
    // reported". It raises `numa_domains` while contributing no node number, so
    // a maximum taken over the rest can under-report and the gap against
    // GetNumaHighestNodeNumber would be filed against the crate -- the R6
    // misattribution reached through `observe`, which is public.
    //
    // It is counted separately from `numa_domains_only_in_cpu_sets` because
    // that count's message names CPU Sets as the reporter, and here nobody was.
    let mut observation = agreeing_observation();
    observation.numa_domains = 2;
    observation.numa_domains_unreported = 1;

    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert_eq!(check.parse_incomplete.len(), 1, "{check:?}");
    assert!(
        !check.parse_incomplete[0].contains("CPU Sets"),
        "nobody reported it, so the message must not name a reporter: {check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
}

#[test]
fn observe_counts_an_unreported_numa_domain_apart_from_a_cpu_sets_only_one() {
    // Drives the extraction, so the two counters cannot silently merge: the
    // predicate for "only CPU Sets" is also satisfied by a domain with no
    // observations at all, and asserting only `!observed_by(walk)` described
    // such a domain as "reported only by CPU Sets".
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Source,
    };

    let memory = || DomainKind::Memory {
        memory_bytes: windows_topology_sys::Observed::NotObserved,
    };
    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: memory(),
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::CpuSets, 1)],
            },
            Domain {
                kind: memory(),
                processors: Default::default(),
                observations: Vec::new(),
            },
        ],
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        Some(1),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.numa_domains, 2);
    assert_eq!(
        observation.numa_domains_only_in_cpu_sets, 1,
        "only the domain CPU Sets actually reported"
    );
    assert_eq!(
        observation.numa_domains_unreported, 1,
        "and the one nobody reported is counted apart from it"
    );
}

#[test]
fn a_contradicted_counter_puts_the_parse_in_doubt_even_with_nothing_else_wrong() {
    // The renderer gates its hardware conclusions on `parse_in_doubt`, and this
    // is the half that was missing. A counter read independently from Windows
    // contradicting the walk is the strongest evidence the parsed list is
    // short: a crate reporting one group where GetActiveProcessorGroupCount
    // says two did not miscount, it never saw the second group's records --
    // including its caches. Yet nothing failed to decode, coherence agrees, and
    // no domain is CPU-Sets-only, so `parse_incomplete` is EMPTY and the
    // condition the renderer used printed "this machine reports no L3 at all"
    // directly above "=> DISAGREE".
    let mut observation = agreeing_observation();
    observation.groups = 1;
    observation.raw_group_count = 2;

    let check = observation.cross_check();
    assert!(
        check.parse_incomplete.is_empty(),
        "the fixture must reach this through disagreements alone, or it proves nothing: {check:?}"
    );
    assert!(check.parse_in_doubt(), "{check:?}");
}

#[test]
fn a_counter_that_could_not_be_read_does_not_put_the_parse_in_doubt() {
    // The other half of `parse_in_doubt`'s claim, and the one that would rot
    // silently: excluding `not_compared` is a decision, not an oversight. A
    // GetActiveProcessorCount that returned its failure sentinel says nothing
    // about the topology parse, so caveating the cache conclusions on it would
    // assert a doubt this run does not have.
    //
    // The verdict still moves -- this run cannot claim agreement -- which is
    // exactly the distinction: the two conditions answer different questions.
    let mut observation = agreeing_observation();
    observation.raw_active_processors = 0;

    let check = observation.cross_check();
    assert!(!check.not_compared.is_empty(), "{check:?}");
    assert!(
        !check.parse_in_doubt(),
        "a counter this probe could not read is not evidence about the parse: {check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "but it does block agreement: {check:?}"
    );
}

#[test]
fn an_uncollected_coherence_blocks_agreement_too() {
    // Unreachable from `discover`, which returns `Agreed` or `Disagreed`. Pinned
    // anyway because the rule that makes this safe is fiat rather than derived:
    // only `Agreed` clears the check, so a variant added to `Coherence` later
    // fails the exhaustive match in `cross_check` rather than quietly becoming a
    // fourth way to reach `Verdict::Agree`.
    let mut observation = agreeing_observation();
    observation.coherence = windows_topology_sys::Coherence::NotCollected;

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
    assert_eq!(check.parse_incomplete.len(), 1, "{check:?}");
}

#[test]
fn a_real_disagreement_still_outranks_an_incomplete_parse() {
    // Both conditions at once. The verdict must stay `Disagree`, because that is
    // the only one that sends a reader to audit the crate's parse -- an
    // incomplete parse must not be able to downgrade a genuine contradiction
    // into a caveat.
    let mut observation = agreeing_observation();
    observation.groups = 9;
    observation.coherence = windows_topology_sys::Coherence::NotCollected;

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Disagree,
        "{check:?}"
    );
    assert_eq!(check.disagreements.len(), 1, "{check:?}");
    assert_eq!(
        check.parse_incomplete.len(),
        1,
        "and the caveat is still reported rather than swallowed: {check:?}"
    );
}

#[test]
fn a_real_disagreement_outranks_a_counter_that_could_not_be_read() {
    // Both at once. The verdict must stay `Disagree`, because a finding about
    // the crate's parse is not softened by an unrelated failed read -- and the
    // skipped counter must still be listed, so the reader knows the
    // disagreement is not the whole picture.
    let mut observation = agreeing_observation();
    observation.raw_highest_numa_node = None;
    observation.groups = 2;

    let check = observation.cross_check();
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Disagree,
        "{check:?}"
    );
    assert_eq!(check.disagreements.len(), 1, "{check:?}");
    assert_eq!(check.not_compared.len(), 1, "{check:?}");
}

#[test]
fn every_core_reports_processors_and_smt_agrees_with_the_count() {
    let observation = crate::topology::measure().expect("topology discovery");

    for core in &observation.cores {
        assert!(
            core.processors >= 1,
            "a core with no processors is a parse error, not a machine"
        );
        assert!(
            !core.contradicts_itself(),
            "SMT is exactly the condition of a core carrying more than one \
             processor, and this record disagrees with itself: {core:?}"
        );
    }
}

#[test]
fn every_cache_level_reports_at_least_one_domain_with_at_least_one_processor() {
    let observation = crate::topology::measure().expect("topology discovery");

    for cache in &observation.caches {
        assert!(
            cache.level >= 1,
            "a cache level of zero is a parse error, not a machine"
        );
        // `domains() >= 1` asserted explicitly, and it is what the test's name
        // promises. Without it the assertion below passes VACUOUSLY on the case
        // worth catching: `all()` over an empty iterator is `true`.
        //
        // That state is reachable rather than theoretical. `measure` builds one
        // summary per entry in `cache_levels()`, which filters on kind alone,
        // while the per-level partition count drops domains covering no
        // processors -- so a level can appear here with no domains at all, and
        // the probe would print `L3  0 domain(s)` while this test, whose whole
        // job is to catch a parsing regression on whatever hardware CI runs,
        // reported success.
        //
        // Two ways in, and they differ in whether anything else notices. A
        // truncated trailing `GROUP_AFFINITY` array raises an anomaly, so
        // `parse_in_doubt` already covers it. A well-formed array whose MASK is
        // zero does not: `read_cache_body` reports only a declared-versus-read
        // count mismatch and never inspects mask contents, and `AnomalyKind`
        // has no variant for it. This comment used to claim a zero mask was
        // recorded as an anomaly, which is why that case went uncaveated until
        // `cross_check` gained an entry for a level with no partitions.
        assert!(
            cache.domains() >= 1,
            "cache level {} reports no domains at all, which is a parse \
             regression rather than a machine",
            cache.level
        );
        assert!(
            cache.processors_per_domain.iter().all(|&span| span >= 1),
            "a cache domain covering no processors is a parse error"
        );
    }
}

#[test]
fn the_outermost_partitioning_cache_is_the_deepest_level_that_splits_the_machine() {
    let observation = crate::topology::measure().expect("topology discovery");

    // What this can check, and deliberately no more. `Observation`'s caches are
    // SUMMARIES -- a level, a domain count, and the size of each domain -- with
    // processor membership discarded. "Outermost" is defined by set inclusion
    // between partitions, so nothing here can re-derive the selection, and the
    // two attempts to do so anyway were both wrong:
    //
    // - Ordering candidates by LEVEL NUMBER is the rule the topology crate
    //   abandoned, because a higher number is not always coarser. This module's
    //   own `outermost_partitioning_cache` doc records it, and the synthetic
    //   test below builds the counterexample: a valid L2 outermost partition
    //   alongside a finer L3 that still partitions. A level-number assertion
    //   here would fail that legitimate topology.
    //
    // - Reading `None` as "no level partitions this machine" is false for the
    //   same reason the renderer was corrected: `None` is also the answer when
    //   two partitionings are incomparable, which is a deliberate ambiguity
    //   result rather than a claim about the hardware.
    //
    // So this asserts the two properties that survive the summarising: a
    // selection is one of the surveyed levels and genuinely partitions, and the
    // selection agrees with the level the survey captured.
    match observation.outermost_partitioning_cache() {
        Some(chosen) => {
            assert!(
                chosen.domains() > 1,
                "a level that does not partition cannot be the partitioning level"
            );
            // Asserted against the RAW fields, because anything asked of
            // `chosen` is answered by how it was obtained. `chosen` is
            // `caches.iter().find(|c| c.level == partitioning_cache_level)`, so
            // "the looked-up summary is the level the survey selected" and "the
            // selected level is one this survey recorded" are both true by
            // construction -- an earlier version asserted exactly those two, and
            // neither could fail. This is the same defect as the `None` arm's,
            // in the same function, and fixing one arm without sweeping the
            // other is how it survived.
            //
            // Exactly one entry at the level is the property that CAN fail:
            // `find` takes the first match, so a duplicated level would make the
            // survey's answer depend on enumeration order while every assertion
            // about `chosen` still passed.
            let at_level = observation
                .caches
                .iter()
                .filter(|c| Some(c.level) == observation.partitioning_cache_level)
                .count();
            assert_eq!(
                at_level, 1,
                "the survey must record exactly one summary for the selected \
                 level; {at_level} would make the lookup order-dependent"
            );
        }
        // `None` must mean the survey captured no level -- and ONLY that.
        //
        // The other way to reach this arm is a survey that DID capture a level
        // whose summary is missing from `caches`, which is a defect in this
        // probe: the level the crate named and the summaries the survey carries
        // have to agree, or every conclusion drawn from `caches` is about a
        // different machine than the one the crate answered about.
        //
        // The justification used to be a renderer string -- that the report
        // "would then print 'no unique outermost partitioning cache was
        // established'". It no longer does: that state is
        // `PartitioningCache::SummaryMissing`, which the renderer prints as its
        // own "BUG IN THIS PROBE" arm, pinned by
        // `a_named_level_with_no_summary_is_a_probe_defect_not_a_fact_about_the_machine`.
        // Naming a downstream behaviour to justify an invariant is what let the
        // comment rot when that behaviour was fixed -- and, worse, gave a reader
        // a true observation ("the renderer already handles this") that argues
        // for deleting the only assertion guarding the invariant itself.
        //
        // Written as one condition on purpose. The previous version asserted
        // `captured.is_none() || summary_is_missing`, which is the disjunction
        // of the two conditions that reach this arm -- unfalsifiable, and it
        // permitted the exact inconsistency its own failure message named.
        None => assert!(
            observation.partitioning_cache_level.is_none(),
            "the survey captured level {:?} and carries no summary for it, so the \
             level the crate named and the summaries reported here disagree",
            observation.partitioning_cache_level
        ),
    }
}

#[test]
fn every_policy_would_produce_at_least_one_domain() {
    // The live host first, which is the case that must never regress.
    let observation = crate::topology::measure().expect("topology discovery");
    assert_every_policy_is_usable(&observation, "the live host");
}

#[test]
fn every_policy_survives_a_topology_that_reported_no_relationships() {
    // The degenerate case, and it needs constructing because a healthy machine
    // cannot produce it. `MachineMemoryTopology::discover` returns `Ok` when a
    // `PROCESSOR_RELATIONSHIP` record is shorter than its declared size: the
    // record is dropped and an enumeration anomaly is recorded, so a topology
    // with no package and no core relationships is a legal result rather than a
    // parse failure.
    //
    // Running this only against `measure()` was why `by-package` and `by-core`
    // could return zero unnoticed -- every real host has both, so the clamp
    // those two policies were missing was never exercised.
    let mut observation = agreeing_observation();
    observation.packages = 0;
    observation.cores = Vec::new();
    observation.numa_domains = 0;
    observation.numa_domains_without_processors = 0;
    observation.caches = Vec::new();
    observation.partitioning_cache_level = None;

    assert_every_policy_is_usable(&observation, "a topology with no relationships");
}

#[test]
fn every_policy_survives_a_selected_cache_level_reporting_no_domains() {
    // The one arm the other fixtures never reach. All of them leave
    // `partitioning_cache_level` as `None`, so `domain_counts` took the `None`
    // default and the selected-value path went untested -- which is how that
    // arm kept returning `c.domains` raw while the comment above it claimed
    // every count was clamped.
    let mut observation = agreeing_observation();
    observation.caches = vec![crate::topology::CacheLevel {
        level: 3,
        processors_per_domain: Vec::new(),
    }];
    observation.partitioning_cache_level = Some(3);

    assert_every_policy_is_usable(&observation, "a selected level reporting no domains");
}

#[test]
fn every_policy_survives_more_processorless_numa_domains_than_domains() {
    // The two counters are read independently, so their relationship is an
    // assumption rather than a guarantee. An unsigned subtraction that trusted
    // it would panic instead of reporting.
    let mut observation = agreeing_observation();
    observation.numa_domains = 1;
    observation.numa_domains_without_processors = 4;

    assert_every_policy_is_usable(&observation, "more processorless domains than domains");
}

/// A fleet sized at zero domains performs no I/O at all, so every policy must
/// name at least one whatever the survey found.
fn assert_every_policy_is_usable(observation: &crate::topology::Observation, what: &str) {
    let counts = observation.domain_counts();

    // The SET first, then the values. Iterating alone passed vacuously on an
    // empty list and on a single bogus entry, so a mutation sweep found
    // `domain_counts` replaceable by `vec![]`, `vec![("", 1)]` and
    // `vec![("xyzzy", 1)]` with every caller still green -- the helper named
    // "every policy" never checked that any policy was there.
    let names: Vec<&str> = counts.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        vec![
            "single",
            "by-package",
            "by-numa-domain-with-processors",
            "by-outermost-partitioning-cache",
            "by-core",
        ],
        "on {what}, the reported policies are not the five this probe surveys"
    );

    for (name, count) in counts {
        assert!(
            count >= 1,
            "on {what}, policy {name} would produce {count} domains, and a fleet \
             of zero domains can perform no I/O"
        );
    }
}

// --- M5+.3: the partitioning rule has one implementation ---

#[test]
fn observe_captures_the_crates_partitioning_level_where_re_derivation_would_differ() {
    // Drives `observe`, which is what the synthetic test below does NOT: that
    // one hand-sets `partitioning_cache_level`, so it pins the lookup and
    // leaves the CAPTURE untested. Swapping `observe`'s call to the crate for
    // the old "highest level with more than one domain" would leave it green
    // while the report selected the wrong level -- the regression `SH-16.9`
    // exists to prevent.
    //
    // Four processors. L2 splits them into two blocks, L3 into four, so L3
    // REFINES L2 and the coarsest -- the crate's answer -- is L2. Ordering by
    // level number answers L3. The two rules disagree here on purpose.
    use windows_topology_sys::{
        CacheKind, Domain, DomainKind, MachineMemoryTopology, Observation, Observed, Processor,
        ProcessorId, Source,
    };

    let cache = |level: u8| DomainKind::Cache {
        level,
        associativity: 8,
        line_size: 64,
        size_bytes: 1 << 20,
        cache_type: CacheKind::Unified,
    };
    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let block = |members: &[u8]| -> windows_topology_sys::ProcessorSet {
        members.iter().map(|n| (0u16, *n)).collect()
    };

    let topology = MachineMemoryTopology {
        processors: (0..4)
            .map(|number| Processor {
                id: ProcessorId { group: 0, number },
                online: true,
                capacity: 0,
            })
            .collect(),
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: block(&[0, 1, 2, 3]),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Memory {
                    memory_bytes: Observed::NotObserved,
                },
                processors: block(&[0, 1, 2, 3]),
                observations: walk(),
            },
            Domain {
                kind: cache(2),
                processors: block(&[0, 1]),
                observations: walk(),
            },
            Domain {
                kind: cache(2),
                processors: block(&[2, 3]),
                observations: walk(),
            },
            Domain {
                kind: cache(3),
                processors: block(&[0]),
                observations: walk(),
            },
            Domain {
                kind: cache(3),
                processors: block(&[1]),
                observations: walk(),
            },
            Domain {
                kind: cache(3),
                processors: block(&[2]),
                observations: walk(),
            },
            Domain {
                kind: cache(3),
                processors: block(&[3]),
                observations: walk(),
            },
        ],
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        4,
        1,
        Some(0),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(
        observation.partitioning_cache_level,
        Some(2),
        "the crate answers L2 as the coarsest partitioning level; ordering by \
         level number would answer L3, and the survey must not re-derive"
    );
}

#[test]
fn the_survey_reports_the_topology_crates_partitioning_level_not_its_own() {
    // The restatement this removes differed from the crate's answer in two
    // ways: it omitted the pairwise-disjointness check, and it ordered
    // candidates by LEVEL NUMBER, which the topology crate stopped doing
    // because a higher number is not always coarser.
    //
    // Here the higher level is the finer partition, so the two rules disagree:
    // the old `max_by_key(level)` would answer L3, and asking the crate answers
    // L2. The survey must report what the crate says.
    let mut observation = agreeing_observation();
    observation.caches = vec![
        crate::topology::CacheLevel {
            level: 2,
            processors_per_domain: vec![2, 2],
        },
        crate::topology::CacheLevel {
            level: 3,
            processors_per_domain: vec![1, 1, 1, 1],
        },
    ];
    observation.partitioning_cache_level = Some(2);

    assert_eq!(
        observation.outermost_partitioning_cache().map(|c| c.level),
        Some(2),
        "the survey must not re-derive; it looks up what the crate decided"
    );
}

#[test]
fn no_partitioning_level_is_a_real_answer_in_the_survey_too() {
    // The fixture's level DOES partition -- four domains -- while the crate
    // captured no level. That combination is what makes this test able to fail:
    // any rule that re-derived the answer from the summaries would see a
    // partitioning level and report L3, so only a survey that genuinely looks
    // up what the crate decided answers `None` here.
    //
    // An earlier version used `domains: 1`. Every re-derivation rule requires
    // `domains > 1`, so a survey that re-derived would also have answered
    // `None`, and the test could not tell the two apart -- it was named for a
    // regression it did not detect.
    let mut observation = agreeing_observation();
    observation.caches = vec![crate::topology::CacheLevel {
        level: 3,
        processors_per_domain: vec![1, 1, 1, 1],
    }];
    observation.partitioning_cache_level = None;

    assert!(
        observation.outermost_partitioning_cache().is_none(),
        "the survey must report the crate's answer, not re-derive one from a \
         level that happens to partition"
    );
}

// --- the report's claims, pinned ---
//
// A mutation sweep over the renderer found 13 of 13 mutants surviving: `render`
// could return "xyzzy" and the suite stayed green. The survivors were exactly
// the claims that cost the most review rounds -- the "no L3 at all" note, the
// caveat gate on an absent partitioning answer, the heterogeneous-core note --
// each of which had been checked by running the binary and reading the output,
// which nothing repeats on a later change.

/// A stand-in for the host fingerprint line.
///
/// The renderer takes it as an argument rather than reading it: `banner_line`
/// runs a topology discovery of its own, so calling it inside `report` made
/// this module's claim to be testable without a host false of its very first
/// line, and made a second platform read that neither bracketed the other.
const BANNER: &str = "host:  TEST-FIXTURE";

/// An observation whose report should be free of every caveat.
fn clean_observation() -> crate::topology::Observation {
    let mut observation = agreeing_observation();
    observation.caches = vec![
        crate::topology::CacheLevel {
            level: 2,
            processors_per_domain: vec![2, 2],
        },
        crate::topology::CacheLevel {
            level: 3,
            processors_per_domain: vec![4],
        },
    ];
    observation.partitioning_cache_level = Some(2);
    observation
}

#[test]
fn the_report_names_the_outermost_partitioning_level_the_crate_chose() {
    let text = crate::topology_report::report(BANNER, &clean_observation());

    assert!(
        text.contains("outermost cache that partitions the processors it covers: L2 (2 domains)"),
        "{text}"
    );
    assert!(
        text.contains(r#""outermost_partitioning_cache":"level""#),
        "{text}"
    );
}

#[test]
fn the_no_l3_note_is_a_hardware_claim_only_when_the_parse_is_whole() {
    // The claim that cost two review rounds. Asserted as hardware when nothing
    // is in doubt, and as a fact about the parse when something is -- otherwise
    // a host whose L3 record alone failed to decode is filed as an ARM64-style
    // no-L3 machine.
    let mut whole = clean_observation();
    whole.caches = vec![crate::topology::CacheLevel {
        level: 2,
        processors_per_domain: vec![2, 2],
    }];
    let text = crate::topology_report::report(BANNER, &whole);
    assert!(
        text.contains("NOTE: this machine reports no L3 at all"),
        "a whole parse with no L3 states the hardware fact: {text}"
    );

    let mut in_doubt = whole.clone();
    in_doubt.enumeration_anomalies = vec![windows_topology_sys::EnumerationAnomaly {
        source: windows_topology_sys::Source::RelationshipWalk,
        offset: 64,
        kind: windows_topology_sys::AnomalyKind::Undersized {
            declared: 8,
            minimum: 48,
        },
    }];
    let text = crate::topology_report::report(BANNER, &in_doubt);
    assert!(
        !text.contains("NOTE: this machine reports no L3 at all"),
        "a parse in doubt must not assert the hardware fact: {text}"
    );
    assert!(
        text.contains("no L3 decoded on this run"),
        "it states what the parse showed instead: {text}"
    );
}

#[test]
fn the_report_caveats_its_cache_conclusions_exactly_when_the_parse_is_in_doubt() {
    let caveat = "This run did not establish that the parse";

    let text = crate::topology_report::report(BANNER, &clean_observation());
    assert!(
        !text.contains(caveat),
        "nothing in doubt, no caveat: {text}"
    );

    // A disagreement, which `parse_in_doubt` includes.
    let mut disagreeing = clean_observation();
    disagreeing.raw_group_count = 2;
    let text = crate::topology_report::report(BANNER, &disagreeing);
    assert!(text.contains(caveat), "{text}");

    // A failed counter read, which it deliberately does NOT: that bears on this
    // probe's ability to check the parse, not on the parse.
    let mut unread = clean_observation();
    unread.raw_active_processors = 0;
    let text = crate::topology_report::report(BANNER, &unread);
    assert!(
        !text.contains(caveat),
        "a counter this probe could not read is not evidence about the parse: {text}"
    );
    assert!(text.contains(r#""cross_check":"incomplete""#), "{text}");
}

#[test]
fn an_absent_partitioning_answer_says_which_absent_answer_it_is() {
    // Three findings that all rendered as `null` before they were told apart.
    let mut none_reported = clean_observation();
    none_reported.caches = Vec::new();
    none_reported.partitioning_cache_level = None;
    let text = crate::topology_report::report(BANNER, &none_reported);
    assert!(
        text.contains("no cache levels were reported at all"),
        "{text}"
    );
    assert!(
        text.contains(r#""outermost_partitioning_cache":"no_levels_reported""#),
        "{text}"
    );

    let mut nothing_partitions = clean_observation();
    nothing_partitions.caches = vec![crate::topology::CacheLevel {
        level: 3,
        processors_per_domain: vec![4],
    }];
    nothing_partitions.partitioning_cache_level = None;
    let text = crate::topology_report::report(BANNER, &nothing_partitions);
    assert!(
        text.contains("no cache level reported more than one domain"),
        "{text}"
    );
    assert!(
        text.contains(r#""outermost_partitioning_cache":"none""#),
        "{text}"
    );

    let mut not_unique = clean_observation();
    not_unique.partitioning_cache_level = None;
    let text = crate::topology_report::report(BANNER, &not_unique);
    assert!(
        text.contains("more than one distinct domain"),
        "and it does not claim the machine IS partitioned: {text}"
    );
    assert!(
        text.contains(r#""outermost_partitioning_cache":"not_unique""#),
        "{text}"
    );
}

#[test]
fn the_heterogeneous_note_appears_only_with_more_than_one_efficiency_class() {
    let note = "heterogeneous";
    let one = crate::topology_report::report(BANNER, &clean_observation());
    assert!(!one.contains(note), "{one}");

    let mut mixed = clean_observation();
    mixed.cores = vec![
        crate::topology::CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 2,
        },
        crate::topology::CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 1,
            processors: 2,
        },
    ];
    let text = crate::topology_report::report(BANNER, &mixed);
    assert!(
        text.contains(note),
        "two classes means an unconstrained thread can land on an efficiency core: {text}"
    );
    assert!(text.contains(r#""efficiency_classes":[0,1]"#), "{text}");
}

#[test]
fn the_json_efficiency_classes_are_the_classes_and_not_how_many() {
    // A plural name over a count is ambiguous in the one way that matters: a
    // single-class host emitted `"efficiency_classes":1`, which reads exactly
    // like a machine whose one class is class 1 -- while the prose above it
    // printed `efficiency classes: [0]`. Same fact, same report, two renderings
    // a consumer cannot reconcile.
    let mut one = clean_observation();
    one.cores = vec![crate::topology::CoreShape {
        simultaneous_multithreading: true,
        efficiency_class: 0,
        processors: 2,
    }];
    let text = crate::topology_report::report(BANNER, &one);
    assert!(
        text.contains(r#""efficiency_classes":[0]"#),
        "the class is 0, and the row says so rather than saying `1`: {text}"
    );
    assert!(
        text.contains("efficiency classes: [0]"),
        "and the prose agrees with it: {text}"
    );

    // A host whose single class is genuinely class 1 must not render the same
    // as the one above -- which is precisely what a count did.
    let mut class_one = clean_observation();
    class_one.cores = vec![crate::topology::CoreShape {
        simultaneous_multithreading: true,
        efficiency_class: 1,
        processors: 2,
    }];
    let text = crate::topology_report::report(BANNER, &class_one);
    assert!(text.contains(r#""efficiency_classes":[1]"#), "{text}");
}

#[test]
fn a_run_that_could_not_measure_still_emits_a_machine_readable_row() {
    // Otherwise a host where discovery FAILED is indistinguishable from a job
    // that never ran the probe, which silently excludes exactly the hosts most
    // worth counting.
    let text = crate::topology_report::report_unmeasured(BANNER, &std::io::Error::other("no"));

    assert!(
        text.contains("MachineMemoryTopology::discover failed"),
        "{text}"
    );
    assert!(text.contains(r#""cross_check":"not_measured""#), "{text}");
}

#[test]
fn every_report_carries_the_banner_and_title() {
    // The banner is what lets a captured report be attributed to a machine, and
    // the taint marker travels with it. Both reports open with it, so neither
    // can be pasted somewhere and compared against anything.
    for text in [
        crate::topology_report::report(BANNER, &clean_observation()),
        crate::topology_report::report_unmeasured(BANNER, &std::io::Error::other("no")),
    ] {
        assert!(
            text.contains(
                "== processor topology, and what each partitioning policy would yield =="
            ),
            "{text}"
        );
        assert_eq!(
            text.lines().next().unwrap_or_default(),
            BANNER,
            "the first line must be the banner the caller supplied: {text}"
        );
    }
}

#[test]
fn the_numa_line_names_domains_only_one_source_reported() {
    let quiet = crate::topology_report::report(BANNER, &clean_observation());
    assert!(!quiet.contains("reported only by CPU Sets"), "{quiet}");

    let mut split = clean_observation();
    split.numa_domains = 2;
    split.numa_domains_only_in_cpu_sets = 1;
    let text = crate::topology_report::report(BANNER, &split);
    assert!(
        text.contains("1 reported only by CPU Sets, never by the relationship walk"),
        "{text}"
    );
    assert!(
        text.contains(r#""numa_domains_only_in_cpu_sets":1"#),
        "{text}"
    );
}

#[test]
fn the_missing_level_caveat_needs_both_doubt_and_an_absent_level() {
    // A conjunction, and each half matters. It reads "the level that would have
    // partitioned this machine is missing", which is a claim about why the
    // answer is absent -- so it must not appear when the answer is present, nor
    // when nothing is in doubt.
    let caveat = "Or the parse is not whole and the level that would have partitioned";

    // In doubt, but a level WAS named: the caveat above it already covers this.
    let mut doubted_with_level = clean_observation();
    doubted_with_level.raw_group_count = 2;
    let text = crate::topology_report::report(BANNER, &doubted_with_level);
    assert!(
        !text.contains(caveat),
        "a named level is not missing, whatever else is in doubt: {text}"
    );

    // A level is absent, but nothing is in doubt: the absence is a finding
    // about the machine, not about the parse.
    let mut absent_but_whole = clean_observation();
    absent_but_whole.partitioning_cache_level = None;
    let text = crate::topology_report::report(BANNER, &absent_but_whole);
    assert!(
        !text.contains(caveat),
        "a whole parse does not blame the parse for the absence: {text}"
    );

    // Both.
    let mut both = clean_observation();
    both.partitioning_cache_level = None;
    both.raw_group_count = 2;
    let text = crate::topology_report::report(BANNER, &both);
    assert!(text.contains(caveat), "{text}");
}

#[test]
fn an_empty_core_or_package_record_blocks_agreement() {
    // The empty-record twins of `numa_domains_without_processors`. A zero
    // affinity mask raises no anomaly, so a spurious empty record inflates the
    // count and the policy derived from it while nothing else notices. The
    // live-host test already calls an empty core a parse error; without this,
    // cross_check would still certify one.
    for (label, mutate) in [
        (
            "core(s) and",
            Box::new(|o: &mut crate::topology::Observation| o.cores_without_processors = 1)
                as Box<dyn Fn(&mut crate::topology::Observation)>,
        ),
        (
            "package(s) cover no processors",
            Box::new(|o: &mut crate::topology::Observation| o.packages_without_processors = 1),
        ),
    ] {
        let mut observation = agreeing_observation();
        mutate(&mut observation);

        let check = observation.cross_check();
        assert!(check.disagreements.is_empty(), "{label}: {check:?}");
        assert!(
            check.parse_incomplete.iter().any(|c| c.contains(label)),
            "{label}: {check:?}"
        );
        assert_eq!(
            check.verdict(),
            crate::topology::Verdict::Incomplete,
            "{label}: {check:?}"
        );
    }
}

#[test]
fn observe_counts_empty_core_and_package_records() {
    // Drives the extraction so the counts cannot be dropped silently.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            // The spurious empty records: a zero affinity mask decodes cleanly.
            Domain {
                kind: DomainKind::Package,
                processors: windows_topology_sys::ProcessorSet::default(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: windows_topology_sys::ProcessorSet::default(),
                observations: walk(),
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.packages, 2, "the empty record is still counted");
    assert_eq!(observation.packages_without_processors, 1);
    assert_eq!(observation.cores_without_processors, 1);
    assert!(observation.cross_check().parse_in_doubt());
}

#[test]
fn a_relation_a_caller_described_is_not_a_measurement() {
    // Provenance per RELATION, not just per topology. `Provenance::Measured` is
    // object-level and the crate is explicit that it permits hand-inserted
    // relations -- `Source::Description` exists for exactly that mixed case --
    // so a measured topology can carry counted relations nobody read from the
    // platform.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let build = |source| MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(source, 0)],
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let described = crate::topology::observe(
        &build(Source::Description),
        1,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );
    assert_eq!(described.described_relations, 1);
    assert!(
        described
            .cross_check()
            .parse_incomplete
            .iter()
            .any(|c| c.contains("described by a caller")),
        "a measured topology carrying a described relation is not all measured: {:?}",
        described.cross_check()
    );

    // The control: the same shape, all walk-sourced, carries no such entry.
    let walked = crate::topology::observe(
        &build(Source::RelationshipWalk),
        1,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );
    assert_eq!(walked.described_relations, 0);
    assert!(
        !walked
            .cross_check()
            .parse_incomplete
            .iter()
            .any(|c| c.contains("described by a caller")),
        "{:?}",
        walked.cross_check()
    );
}

#[test]
fn a_bracket_left_open_is_not_the_same_as_a_machine_that_held_still() {
    use crate::topology::{BracketOutcome, bracket_outcome};

    // Three outcomes, not two: an open bracket establishes neither change nor
    // stability, so comparing anyway could file a concurrent hot-add against
    // the parse. One value rather than two flags, because "changed, and also
    // held still" is meaningless and two booleans can be set to say it.
    let good = (4u32, 1u16, Some(0u32));
    assert_eq!(bracket_outcome(good, good), BracketOutcome::HeldStill);
    assert_eq!(
        bracket_outcome(good, (8, 1, Some(0))),
        BracketOutcome::Changed
    );

    for open in [(0u32, 1u16, Some(0u32)), (4, 0, Some(0)), (4, 1, None)] {
        assert_eq!(
            bracket_outcome(open, good),
            BracketOutcome::NotEstablished,
            "a bracket open at one end establishes neither: {open:?}"
        );
    }

    // And the observation says which of the three it was.
    let mut open = agreeing_observation();
    open.bracket = crate::topology::BracketOutcome::NotEstablished;
    let check = open.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(
        check
            .not_compared
            .iter()
            .any(|c| c.contains("bracket around the parse was not closed")),
        "{check:?}"
    );
    assert!(
        !check.parse_in_doubt(),
        "still not a claim about the parse: {check:?}"
    );
}

#[test]
fn observe_will_not_claim_a_bracket_it_was_not_given() {
    // The bracket is the CALLER's fact. `observe` set it to `HeldStill` itself,
    // which claims "every counter was read twice and none moved" -- something it
    // never establishes, having been handed one set of counters and no way to
    // know what produced them. A caller's stale pair could then reach `Agree`,
    // or be filed against the parse as a `Disagree`.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let unbracketed = crate::topology::observe(
        &topology,
        1,
        1,
        None,
        crate::topology::BracketOutcome::NotEstablished,
    );
    let check = unbracketed.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "an unestablished bracket is not a finding about the parse: {check:?}"
    );
    assert!(
        check
            .not_compared
            .iter()
            .any(|c| c.contains("bracket around the parse was not closed")),
        "{check:?}"
    );
    assert_ne!(
        check.verdict(),
        crate::topology::Verdict::Agree,
        "and it cannot certify agreement: {check:?}"
    );
}

#[test]
fn a_relation_no_source_reported_is_counted_whatever_its_kind() {
    // Checked only inside the memory arm, so a hand-inserted CORE, PACKAGE or
    // CACHE with no observations was counted -- and could change a policy or the
    // cache partitioning -- with every quality check still clear.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            // A cache nobody reported. Not a memory domain, so the old check
            // could not see it -- yet it joins `caches` and can move the
            // partitioning answer.
            Domain {
                kind: DomainKind::Cache {
                    level: 2,
                    associativity: 8,
                    line_size: 64,
                    size_bytes: 1 << 20,
                    cache_type: windows_topology_sys::CacheKind::Unified,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: Vec::new(),
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.unreported_relations, 1);
    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("carry no observation from any source")),
        "{check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
}

#[test]
fn a_measured_topology_reporting_no_processors_or_groups_blocks_agreement() {
    // Zero is how BOTH raw counters report failure, so a zero parse beside a
    // failed read was filed only as `not_compared`: `parse_in_doubt` stayed
    // false and the report was free to make uncaveated hardware claims about a
    // machine its own parse says has no processors at all. The guard on the
    // package and core checks reads `online_processors > 0`, so the impossible
    // case silently switched those off as well.
    for (label, mutate) in [
        (
            "no online processors",
            Box::new(|o: &mut crate::topology::Observation| {
                o.online_processors = 0;
                o.raw_active_processors = 0;
            }) as Box<dyn Fn(&mut crate::topology::Observation)>,
        ),
        (
            "no processor groups",
            Box::new(|o: &mut crate::topology::Observation| {
                o.groups = 0;
                o.raw_group_count = 0;
            }),
        ),
    ] {
        let mut observation = agreeing_observation();
        mutate(&mut observation);

        let check = observation.cross_check();
        assert!(
            check.disagreements.is_empty(),
            "{label}: a counter that could not be read is not the crate contradicting it: \
             {check:?}"
        );
        assert!(
            check.parse_incomplete.iter().any(|c| c.contains(label)),
            "{label}: {check:?}"
        );
        assert_eq!(
            check.verdict(),
            crate::topology::Verdict::Incomplete,
            "{label}: {check:?}"
        );
        assert!(
            check.parse_in_doubt(),
            "{label}: so the renderer caveats what it says about the hardware: {check:?}"
        );
    }
}

#[test]
fn both_absent_counts_are_named_together_rather_than_one_standing_for_the_pair() {
    // The rule is stated over the LIST, so a topology missing both must name
    // both. Reporting only the first would leave the second to be discovered by
    // fixing the first and running again.
    let mut observation = agreeing_observation();
    observation.online_processors = 0;
    observation.raw_active_processors = 0;
    observation.groups = 0;
    observation.raw_group_count = 0;

    let check = observation.cross_check();
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("no online processors and processor groups")),
        "{check:?}"
    );
}

#[test]
fn a_topology_nobody_measured_is_not_accused_of_describing_no_machine() {
    // The premise of the rule above is that a RUNNING machine was read. A
    // hand-built topology carrying no processors is not describing a machine
    // wrongly; it is not describing one at all, which
    // `topology_was_measured` already reports.
    let mut observation = agreeing_observation();
    observation.topology_was_measured = false;
    observation.online_processors = 0;
    observation.raw_active_processors = 0;
    observation.groups = 0;
    observation.raw_group_count = 0;

    let check = observation.cross_check();
    assert!(
        !check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("cannot have none")),
        "an unmeasured topology is not held to what a running machine must have: {check:?}"
    );
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("was not measured from a running machine")),
        "and the reason it is exempt is itself reported: {check:?}"
    );
}

#[test]
fn two_walk_records_of_one_kind_claiming_a_processor_block_agreement() {
    // A logical processor belongs to exactly one physical package and exactly
    // one core, so two walk records covering it cannot both describe this
    // machine. Nothing else reaches it: no raw counter measures packages or
    // cores, an overlapping record raises no enumeration anomaly, and both
    // records are non-empty, so the `*_without_processors` counts stay clear
    // while `packages` and `by-core` hold the duplicate.
    let mut observation = agreeing_observation();
    observation.overlapping_walk_relations = 2;

    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("share a processor with another")),
        "{check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "{check:?}"
    );
    assert!(check.parse_in_doubt(), "{check:?}");
}

#[test]
fn observe_counts_walk_packages_that_claim_the_same_processor() {
    // Drives the extraction, and pins the EXCLUSION that makes the rule honest:
    // a core only CPU Sets described overlaps a walk core by design -- the crate
    // keeps both groupings deliberately -- and `cores_only_in_cpu_sets` is the
    // count that reports it. Folding it in here would report one disagreement
    // twice.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let core = |smt: bool| DomainKind::Core {
        simultaneous_multithreading: smt,
        efficiency_class: 0,
    };
    let topology = MachineMemoryTopology {
        processors: (0..2)
            .map(|number| Processor {
                id: ProcessorId { group: 0, number },
                online: true,
                capacity: 0,
            })
            .collect(),
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            // Two packages, both from the walk, both claiming processor 1.
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 1u8)].into_iter().collect(),
                observations: walk(),
            },
            // One core from the walk, and a differently-grouped core only CPU
            // Sets described. They overlap, and that is not this rule's subject.
            Domain {
                kind: core(true),
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: core(false),
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::CpuSets, 0)],
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        2,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(
        observation.overlapping_walk_relations, 2,
        "both packages are counted -- neither is the one that is right"
    );
    assert_eq!(
        observation.cores_only_in_cpu_sets, 1,
        "and the overlapping core is reported as the source disagreement it is"
    );
    assert_eq!(
        observation.packages_without_processors, 0,
        "no record is empty, which is why the existing counts do not see this"
    );
    assert!(observation.cross_check().parse_in_doubt());
}

#[test]
fn observe_counts_walk_cores_that_claim_the_same_processor() {
    // The core half of the same rule, so it cannot be dropped for one kind
    // while the package half keeps passing.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let core = |smt: bool| DomainKind::Core {
        simultaneous_multithreading: smt,
        efficiency_class: 0,
    };
    let topology = MachineMemoryTopology {
        processors: (0..2)
            .map(|number| Processor {
                id: ProcessorId { group: 0, number },
                online: true,
                capacity: 0,
            })
            .collect(),
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: core(true),
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: core(false),
                processors: [(0u16, 1u8)].into_iter().collect(),
                observations: walk(),
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        2,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.overlapping_walk_relations, 2);
    assert_eq!(
        observation.cores_only_in_cpu_sets, 0,
        "both came from the walk, so this is not a disagreement between sources"
    );
}

#[test]
fn walk_records_that_share_no_processor_are_not_counted_as_overlapping() {
    // The negative case, so the rule cannot be satisfied by counting every
    // record of a kind that has more than one.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let topology = MachineMemoryTopology {
        processors: (0..2)
            .map(|number| Processor {
                id: ProcessorId { group: 0, number },
                online: true,
                capacity: 0,
            })
            .collect(),
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 1u8)].into_iter().collect(),
                observations: walk(),
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        2,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(observation.packages, 2);
    assert_eq!(observation.overlapping_walk_relations, 0);
}

/// A host fingerprint to bracket a measurement with, with `processors` the one
/// field a caller varies to make two readings differ.
///
/// Internally consistent, deliberately: two logical processors per core so
/// `smt` is true by the owning crate's definition, and two cache domains so a
/// `partitioning_cache_level` that claims to divide the machine actually does.
/// The earlier version set four cores over four processors with `smt: true` and
/// a single cache domain at a partitioning level -- a host that cannot exist,
/// in the fixture for the code that reports on hosts.
fn fingerprint(processors: usize) -> windows_placement_probe::fingerprint::Fingerprint {
    let half = processors / 2;
    windows_placement_probe::fingerprint::Fingerprint {
        arch: "test-arch",
        processors,
        cores: half,
        smt: true,
        partitioning_cache_level: Some(2),
        cache_domain_sizes: vec![half, half],
        efficiency_classes: vec![(0, processors)],
        numa_node_sizes: vec![processors],
        provenance: windows_topology_sys::Provenance::Measured,
    }
}

#[test]
fn an_unchanged_host_prints_one_banner_line() {
    // The equal case must render exactly as `banner_line` always did, so every
    // fingerprint string already recorded elsewhere stays comparable with this
    // probe's -- and so this probe's banner matches every other probe's.
    let text = crate::topology_report::attribution(&Ok(fingerprint(8)), &Ok(fingerprint(8)));
    assert_eq!(
        text,
        windows_placement_probe::fingerprint::banner_line_for(&Ok(fingerprint(8))),
        "rendered through the one place that owns the format, not a copy: {text}"
    );
    assert_eq!(text.lines().count(), 1, "{text}");
}

#[test]
fn a_host_that_changed_across_the_run_says_so_and_keeps_both_readings() {
    // The fingerprint is a topology rendering, not a name, so two readings that
    // disagree mean neither identifies the machine the body describes. Both are
    // kept: which one is stale is exactly what cannot be known here.
    let text = crate::topology_report::attribution(&Ok(fingerprint(8)), &Ok(fingerprint(4)));
    assert!(text.contains("8p/"), "{text}");
    assert!(text.contains("4p/"), "{text}");
    assert!(text.contains("HOST READINGS DISAGREE"), "{text}");
    assert!(
        !text.contains("HOST NOT ESTABLISHED"),
        "two readings that were both MADE did establish a difference: {text}"
    );
    assert!(
        !text.contains("CHANGED"),
        "a difference does not establish that the HOST changed -- discovery \
         returns Ok on a parse that dropped a record, so the enumeration may \
         simply have been flaky: {text}"
    );
}

#[test]
fn a_failed_host_read_is_not_reported_as_a_host_that_changed() {
    // Compared as rendered strings, a failed read is a line like any other, so
    // one failure beside one success -- or two failures whose `io::Error` text
    // differs -- read as a machine that moved. That is a claim about the host
    // drawn from a gap in the measurement, which is the inversion this probe
    // exists to avoid: a failed read establishes neither that the host moved
    // nor that it held still.
    let failed = || Err(std::io::Error::other("discovery failed"));
    for (label, before, after) in [
        ("failed first", failed(), Ok(fingerprint(8))),
        ("failed second", Ok(fingerprint(8)), failed()),
        ("failed both", failed(), failed()),
    ] {
        let text = crate::topology_report::attribution(&before, &after);
        assert!(
            !text.contains("HOST READINGS DISAGREE"),
            "{label}: nothing established that the host changed: {text}"
        );
        assert!(
            text.contains("HOST NOT ESTABLISHED"),
            "{label}: and the gap is reported rather than passed off as a host: {text}"
        );
    }
}

#[test]
fn two_failed_host_reads_with_different_messages_are_still_not_a_change() {
    // The case the string comparison got wrong most quietly: both readings
    // failed, so nothing about the machine was established at all, yet the two
    // rendered lines differ because the errors do.
    let text = crate::topology_report::attribution(
        &Err(std::io::Error::other("first")),
        &Err(std::io::Error::other("second")),
    );
    assert!(!text.contains("HOST READINGS DISAGREE"), "{text}");
    assert!(text.contains("HOST NOT ESTABLISHED"), "{text}");
    // Both failed, so the sentence must not say "one of" them did. The arm is
    // shared with the two one-sided cases, which is how the narrower wording
    // came to cover this one.
    assert!(
        text.contains("at least one of the two readings"),
        "the message does not claim a more specific state than the run established: {text}"
    );
}

#[test]
fn a_count_holding_relations_no_platform_reported_cannot_contradict_a_counter() {
    // `observe` is public and takes any topology, so a caller's described
    // relation -- or one nobody reported -- raises `groups`, or a NUMA label,
    // that no platform API ever produced. Comparing that against the counter
    // reads the difference as the shipping crate contradicting Windows, and
    // `verdict` gives `Disagree` precedence, so the report would lead with an
    // accusation this run did not establish.
    for (label, mutate) in [
        (
            "not measured",
            Box::new(|o: &mut crate::topology::Observation| o.topology_was_measured = false)
                as Box<dyn Fn(&mut crate::topology::Observation)>,
        ),
        (
            "described",
            Box::new(|o: &mut crate::topology::Observation| o.described_relations = 1),
        ),
        (
            "unreported",
            Box::new(|o: &mut crate::topology::Observation| o.unreported_relations = 1),
        ),
    ] {
        let mut observation = agreeing_observation();
        mutate(&mut observation);
        // A group the platform never reported, which the counter cannot match.
        observation.groups = 2;

        assert!(
            observation.counts_include_unparsed_relations(),
            "{label}: the predicate is what the gate reads: {observation:?}"
        );
        let check = observation.cross_check();
        assert!(
            check.disagreements.is_empty(),
            "{label}: a count the platform did not produce is not the crate contradicting a \
             counter: {check:?}"
        );
        assert!(
            check
                .not_compared
                .iter()
                .any(|c| c.contains("could not be attributed to the parse")),
            "{label}: and the reason no comparison was made is reported: {check:?}"
        );
        assert_eq!(
            check.verdict(),
            crate::topology::Verdict::Incomplete,
            "{label}: {check:?}"
        );
    }
}

#[test]
fn a_wholly_parsed_topology_is_still_compared_against_its_counters() {
    // The negative case, so the gate above cannot be satisfied by never
    // comparing anything. A real disagreement must still reach `Disagree`.
    let mut observation = agreeing_observation();
    observation.groups = 2;

    assert!(!observation.counts_include_unparsed_relations());
    let check = observation.cross_check();
    assert!(
        check.disagreements.iter().any(|c| c.contains("groups:")),
        "{check:?}"
    );
    assert_eq!(check.verdict(), crate::topology::Verdict::Disagree);
}

#[test]
fn a_core_whose_smt_flag_contradicts_its_own_processor_count_blocks_agreement() {
    // The owning crate defines the flag as "whether this core has more than one
    // logical processor", so the two sit beside each other in one record and can
    // be checked against each other. Nothing else reaches it: no counter
    // measures cores, and a contradictory flag raises no enumeration anomaly.
    //
    // The asserted live-host test already held the parse to this while
    // `cross_check` did not, so the probe could print that every check it could
    // make had matched on a host whose own test had just gone red -- and CI now
    // prints the report even when the tests fail, which is when it gets read.
    for (label, core) in [
        (
            "flag says no, membership says yes",
            crate::topology::CoreShape {
                simultaneous_multithreading: false,
                efficiency_class: 0,
                processors: 2,
            },
        ),
        (
            "flag says yes, membership says no",
            crate::topology::CoreShape {
                simultaneous_multithreading: true,
                efficiency_class: 0,
                processors: 1,
            },
        ),
    ] {
        assert!(
            core.contradicts_itself(),
            "{label}: the predicate both the test and cross_check read: {core:?}"
        );

        let mut observation = agreeing_observation();
        observation.cores = vec![core];

        let check = observation.cross_check();
        assert!(check.disagreements.is_empty(), "{label}: {check:?}");
        assert!(
            check
                .parse_incomplete
                .iter()
                .any(|c| c.contains("disagrees with the number of processors")),
            "{label}: {check:?}"
        );
        assert_eq!(
            check.verdict(),
            crate::topology::Verdict::Incomplete,
            "{label}: {check:?}"
        );
    }
}

#[test]
fn a_core_whose_smt_flag_matches_its_membership_is_not_a_finding() {
    // The negative case, so the rule cannot be satisfied by calling every core
    // contradictory. Both consistent shapes must pass.
    for core in [
        crate::topology::CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 2,
        },
        crate::topology::CoreShape {
            simultaneous_multithreading: false,
            efficiency_class: 0,
            processors: 1,
        },
    ] {
        assert!(!core.contradicts_itself(), "{core:?}");

        let mut observation = agreeing_observation();
        observation.cores = vec![core];
        assert_eq!(
            observation.cross_check().verdict(),
            crate::topology::Verdict::Agree,
            "{core:?}"
        );
    }
}

#[test]
fn a_cache_level_numbered_zero_blocks_agreement() {
    // Windows numbers cache levels from 1, so a level of 0 is a level the parse
    // did not read rather than one the machine has. The asserted live-host test
    // held the parse to this too, and `cross_check` did not.
    let mut observation = agreeing_observation();
    observation.caches = vec![crate::topology::CacheLevel {
        level: 0,
        processors_per_domain: vec![4],
    }];

    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("numbered 0")),
        "{check:?}"
    );
    assert_eq!(check.verdict(), crate::topology::Verdict::Incomplete);
}

#[test]
fn a_relation_the_walk_reported_is_not_counted_as_one_a_caller_described() {
    // `observed_by(Description)` also matched a relation the walk reported and a
    // caller then annotated -- whose membership IS platform-backed -- so the
    // entry this feeds claimed it was "described by a caller rather than
    // reported by any platform API" of a relation the platform had reported.
    // Every observation must be a `Description` for that sentence to be true.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            // Reported by the walk AND annotated by a caller. Platform-backed.
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![
                    Observation::new(Source::RelationshipWalk, 0),
                    Observation::new(Source::Description, 0),
                ],
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        None,
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(
        observation.described_relations, 0,
        "the walk reported it, so it is not a relation only a caller described"
    );
    assert!(
        !observation.counts_include_unparsed_relations(),
        "and it does not suppress the counter comparisons"
    );
}

#[test]
fn a_caller_described_numa_label_is_not_filed_against_the_win32_counter() {
    // A memory domain the walk reported AND a caller annotated is
    // platform-backed, so `described_relations` does not count it and the
    // provenance gate stays open. If the caller's label then joined the maximum,
    // it would be compared against `GetNumaHighestNodeNumber` and the difference
    // filed as a `disagreement` -- an accusation against the shipping parse for
    // a node number no platform API ever produced.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![Observation::new(Source::RelationshipWalk, 0)],
            },
            // The walk says node 0. A caller says node 7 about the same domain.
            Domain {
                kind: DomainKind::Memory {
                    memory_bytes: windows_topology_sys::Observed::NotObserved,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: vec![
                    Observation::new(Source::RelationshipWalk, 0),
                    Observation::new(Source::Description, 7),
                ],
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        1,
        1,
        Some(0),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(
        observation.highest_numa_node,
        Some(0),
        "the maximum reads what the PLATFORM reported, not what a caller added"
    );
    assert!(
        !observation.counts_include_unparsed_relations(),
        "the domain is platform-backed, so the gate is open and the comparison IS made"
    );

    let check = observation.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "so nothing is filed against the parse: {check:?}"
    );
    assert_ne!(
        check.verdict(),
        crate::topology::Verdict::Disagree,
        "the subject is the absent accusation, not this minimal fixture's other \
         complaints -- it carries no cache survey and no coherence: {check:?}"
    );
}

#[test]
fn the_heterogeneity_conclusion_is_caveated_when_the_parse_is_in_doubt() {
    // "An unconstrained thread can land on an efficiency core" is a claim about
    // the hardware, so it is gated on the same `parse_in_doubt` as every other
    // one the renderer draws. It was printed before that condition was even
    // computed, which made it the single exception to a rule the design note
    // states without one -- and the exception was the conclusion drawn from the
    // one field the two sources are known to contradict each other about.
    let mut mixed = clean_observation();
    mixed.cores = vec![
        crate::topology::CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 0,
            processors: 2,
        },
        crate::topology::CoreShape {
            simultaneous_multithreading: true,
            efficiency_class: 1,
            processors: 2,
        },
    ];

    let whole = crate::topology_report::report(BANNER, &mixed);
    assert!(whole.contains("heterogeneous"), "{whole}");
    assert!(
        !whole.contains("the classes"),
        "a whole parse states it without a caveat: {whole}"
    );

    // The sources disagreed about exactly this field.
    mixed.processor_attribute_conflicts = 1;
    assert!(mixed.cross_check().parse_in_doubt());

    let disputed = crate::topology_report::report(BANNER, &mixed);
    assert!(
        disputed.contains("heterogeneous"),
        "the fact is still reported: {disputed}"
    );
    assert!(
        disputed.contains("did not establish that the parse is whole"),
        "but it is no longer stated as a settled fact about the machine: {disputed}"
    );
}

#[test]
fn observe_counts_walk_numa_nodes_that_claim_the_same_processor() {
    // A processor belongs to exactly one NUMA node, just as it belongs to one
    // package and one core, so a walk reporting it in two nodes describes no
    // machine -- and `numa_domains` feeds a policy directly. The overlap rule
    // was written as two `+`-joined calls naming Package and Core, which is the
    // enumerate-the-kinds shape this file has been bitten by before; memory was
    // the kind left out.
    //
    // The highest-label comparison cannot stand in for it: two overlapping
    // nodes can carry any labels at all, including the correct maximum, which
    // is what this fixture uses.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Observed, Processor, ProcessorId,
        Provenance, Source,
    };

    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let memory = || DomainKind::Memory {
        memory_bytes: Observed::NotObserved,
    };
    let topology = MachineMemoryTopology {
        processors: (0..2)
            .map(|number| Processor {
                id: ProcessorId { group: 0, number },
                online: true,
                capacity: 0,
            })
            .collect(),
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: true,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            // Two NUMA nodes from the walk, both claiming processor 1. Both
            // labelled 0, so the highest-node comparison still matches.
            Domain {
                kind: memory(),
                processors: [(0u16, 0u8), (0, 1)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: memory(),
                processors: [(0u16, 1u8)].into_iter().collect(),
                observations: walk(),
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    let observation = crate::topology::observe(
        &topology,
        2,
        1,
        Some(0),
        crate::topology::BracketOutcome::HeldStill,
    );

    assert_eq!(
        observation.overlapping_walk_relations, 2,
        "both nodes are counted -- neither is the one that is right"
    );
    assert_eq!(
        observation.highest_numa_node,
        Some(0),
        "and the counter comparison is satisfied, which is why it cannot catch this"
    );

    let check = observation.cross_check();
    assert!(
        check.disagreements.is_empty(),
        "the counter agreed, so nothing is filed against the parse: {check:?}"
    );
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("share a processor with another")),
        "{check:?}"
    );
    assert!(check.parse_in_doubt(), "{check:?}");
}

#[test]
fn a_conflict_count_says_what_it_counted_rather_than_which_source_said_it() {
    // Both predicates look for more than one DISTINCT VALUE and neither groups
    // the claims by the API that issued them, so "the two sources disagreed" is
    // more than either establishes. It is the usual cause on the path `measure`
    // takes, and it is not what was checked.
    for (label, mutate) in [
        (
            "distinct node number",
            Box::new(|o: &mut crate::topology::Observation| {
                o.numa_domains_with_conflicting_labels = 1;
            }) as Box<dyn Fn(&mut crate::topology::Observation)>,
        ),
        (
            "distinct attribute value",
            Box::new(|o: &mut crate::topology::Observation| {
                o.processor_attribute_conflicts = 1;
            }),
        ),
    ] {
        let mut observation = agreeing_observation();
        mutate(&mut observation);

        let check = observation.cross_check();
        let entries = check.parse_incomplete.join(" ");
        assert!(
            entries.contains("more than one distinct"),
            "{label}: states what it counted: {check:?}"
        );
        assert!(
            !entries.contains("the two sources") && !entries.contains("from each source"),
            "{label}: and does not name a source split it never made: {check:?}"
        );
    }
}

#[test]
fn a_single_domain_at_a_level_is_reported_as_a_finding_not_as_hardware() {
    // "No cache boundary divides the work" is a claim about the silicon. A
    // level whose one domain covers half the online processors lands in the
    // same arm, and nothing here checks that a level's domains cover the
    // machine -- so the sentence states what this report found instead.
    let mut observation = clean_observation();
    observation.online_processors = 4;
    observation.partitioning_cache_level = None;
    observation.caches = vec![crate::topology::CacheLevel {
        level: 2,
        processors_per_domain: vec![2],
    }];

    let text = crate::topology_report::report(BANNER, &observation);
    assert!(
        text.contains("nothing here divides"),
        "the finding is about this report: {text}"
    );
    assert!(
        !text.contains("no cache boundary"),
        "not about the machine, whose cache coverage this run never checked: {text}"
    );
}

#[test]
fn a_bracket_outcome_reaches_the_observation_the_caller_passed_it() {
    // `observe` takes the bracket as an ARGUMENT and stores what the caller
    // passed. The field's doc said `observe` "leaves it `false`" -- true while
    // the field was a `bool`, and left behind by the change to `BracketOutcome`,
    // so it named a value the type no longer has for a parameter the function
    // now takes. Pinned so the doc and the signature cannot drift apart again.
    use windows_topology_sys::{
        Domain, DomainKind, MachineMemoryTopology, Observation, Processor, ProcessorId, Provenance,
        Source,
    };

    let walk = || vec![Observation::new(Source::RelationshipWalk, 0)];
    let topology = MachineMemoryTopology {
        processors: vec![Processor {
            id: ProcessorId {
                group: 0,
                number: 0,
            },
            online: true,
            capacity: 0,
        }],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Package,
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(0u16, 0u8)].into_iter().collect(),
                observations: walk(),
            },
        ],
        provenance: Provenance::Measured,
        ..MachineMemoryTopology::default()
    };

    for outcome in [
        crate::topology::BracketOutcome::HeldStill,
        crate::topology::BracketOutcome::Changed,
        crate::topology::BracketOutcome::NotEstablished,
    ] {
        let observation = crate::topology::observe(&topology, 1, 1, None, outcome);
        assert_eq!(
            observation.bracket, outcome,
            "observe stores the caller's outcome rather than deciding one"
        );
    }
}

#[test]
fn a_named_level_with_no_summary_blocks_agreement_and_sizes_to_one_domain() {
    // The renderer prints this state as "BUG IN THIS PROBE ... Nothing below
    // about cache partitioning can be trusted", and `cross_check` said nothing
    // about it -- so the verdict could certify the same run as `agree`, two
    // paragraphs apart on one page.
    //
    // `domain_counts` also read it through `outermost_partitioning_cache`,
    // whose `None` folds four distinct answers together, so a sizing policy
    // silently got "one domain" from a state the report calls a bug.
    let mut observation = agreeing_observation();
    observation.caches = vec![crate::topology::CacheLevel {
        level: 2,
        processors_per_domain: vec![2, 2],
    }];
    // A level the survey has no summary for.
    observation.partitioning_cache_level = Some(3);

    assert_eq!(
        observation.partitioning_cache(),
        crate::topology::PartitioningCache::SummaryMissing(3)
    );

    let check = observation.cross_check();
    assert!(check.disagreements.is_empty(), "{check:?}");
    assert!(
        check
            .parse_incomplete
            .iter()
            .any(|c| c.contains("carries no summary for it")),
        "{check:?}"
    );
    assert_eq!(
        check.verdict(),
        crate::topology::Verdict::Incomplete,
        "the verdict cannot say `agree` beside a report that says BUG: {check:?}"
    );

    // And the sizing fallback is still one domain -- stated per variant rather
    // than inherited from a `None`, so a new variant is a compile error.
    let counts = observation.domain_counts();
    let (_, cache_policy) = counts
        .iter()
        .find(|(name, _)| *name == "by-outermost-partitioning-cache")
        .expect("the policy is in the table");
    assert_eq!(*cache_policy, 1);

    // The report says so too, and the two agree.
    let text = crate::topology_report::report(BANNER, &observation);
    assert!(text.contains("BUG IN THIS PROBE"), "{text}");
    assert!(
        text.contains("=> INCOMPLETE"),
        "the prose verdict agrees with the bug note rather than saying agree: {text}"
    );
    assert!(
        text.contains(r#""cross_check":"incomplete""#),
        "and the mining row says the same: {text}"
    );
}

#[test]
fn a_named_level_with_no_summary_is_not_also_called_missing() {
    // Two contradictions in one arm, both introduced by the fix that made
    // `SummaryMissing` block agreement.
    //
    // The prose says "the topology crate named L3 as the outermost partitioning
    // cache", and the caveat gate -- which excludes only `Level(_)` -- then
    // added "the level that would have partitioned this machine is missing"
    // directly beneath it. A level was named; it is its SUMMARY that is absent.
    // `the_missing_level_caveat_needs_both_doubt_and_an_absent_level` already
    // states that invariant for `Level`, and `SummaryMissing` names a level too.
    //
    // The NDJSON separately laundered the level away: it read through
    // `outermost_partitioning_cache()`, whose `None` covers this case, so the
    // row carried `"outermost_partitioning_cache_level":null` beside
    // `"outermost_partitioning_cache":"summary_missing"` while the prose printed
    // the number. `domain_counts` was de-laundered in the same commit that
    // created the contradiction; this consumer was not swept with it.
    let mut observation = agreeing_observation();
    observation.caches = vec![crate::topology::CacheLevel {
        level: 2,
        processors_per_domain: vec![2, 2],
    }];
    observation.partitioning_cache_level = Some(3);
    assert_eq!(
        observation.partitioning_cache(),
        crate::topology::PartitioningCache::SummaryMissing(3)
    );
    assert!(observation.cross_check().parse_in_doubt());

    let text = crate::topology_report::report(BANNER, &observation);
    assert!(text.contains("named L3"), "{text}");
    assert!(
        !text.contains("the level that would have partitioned this"),
        "a level WAS named -- its summary is what is absent, and the two \
         statements cannot both be printed: {text}"
    );
    assert!(
        text.contains(r#""outermost_partitioning_cache_level":3"#),
        "the row carries the level the prose names, rather than laundering it \
         to null through an Option accessor: {text}"
    );
    assert!(
        text.contains(r#""outermost_partitioning_cache":"summary_missing""#),
        "and still says which absent case it is: {text}"
    );
}

// --- the doorbell's park-and-wake handshake ---------------------------------
//
// Its own documentation records that the first implementation DEADLOCKED: one
// thread setting an auto-reset event while the other waited, two signals
// collapsing into one, and the waiter blocking on INFINITE for ever. Nothing
// tested it, so the rewrite that fixed it could have been undone silently.
//
// Both tests are bounded by construction. A test that can hang is worse than
// the defect it guards, because it takes the whole suite with it: the handshake
// itself waits with a 5-second timeout and reports `None` rather than blocking,
// so a reintroduced deadlock surfaces here as a failed assertion within seconds
// rather than as a suite that never finishes.

#[test]
fn a_zero_round_handshake_has_no_average_rather_than_a_meaningless_one() {
    // The deterministic half of the contract, and the one a caller is most
    // likely to break by "simplifying". With no rounds the elapsed time is zero
    // and the average would be `0.0 / 0.0` -- NaN, wrapped in the `Some` this
    // function documents as a meaningful number, against which every comparison
    // a caller makes returns false with no indication why.
    assert_eq!(
        crate::doorbell_cost::measure_park_and_wake(0),
        None,
        "zero rounds has no average, and must not report one"
    );
}

#[test]
fn a_small_handshake_completes_and_reports_a_positive_round_trip() {
    // The liveness half: the two-event alternation actually runs to completion
    // and produces a number. Deliberately few rounds -- this is a real
    // cross-thread measurement, and the assertion is that it terminates and is
    // sane, not that it is fast.
    //
    // The tolerance for a loaded machine is large but **bounded**, and this once
    // said "arbitrarily slow without making it wrong", which is not true of the
    // code it describes. Each wait inside the handshake carries a 5-second
    // timeout; a round that exceeds it makes `measure_park_and_wake` return
    // `None` and fails the `expect` below. The section header above states that
    // bound correctly and the `expect` message names it outright, so the claim
    // was contradicted twice within a few lines of making it.
    //
    // Bounded is the deliberate choice, for the reason in that header: a test
    // that can hang takes the whole suite with it, which is worse than the
    // defect it guards against. The margin is enormous -- a round trip is
    // sub-microsecond in practice against a 5-second ceiling -- so a failure
    // here means the machine stalled for seconds, which is worth a red test
    // rather than a silently slow pass.
    let average = crate::doorbell_cost::measure_park_and_wake(64)
        .expect("a bounded handshake of 64 rounds must complete rather than time out");

    assert!(
        average.is_finite() && average > 0.0,
        "a completed handshake must report a positive finite round trip, got {average}"
    );
}

// --- what `prepare` needs from a drive letter -------------------------------
//
// `request_cost::measure` builds its long-path sample on a hard-coded `C:`, and
// two review passes read that as a portability bug: a machine with no `C:`
// volume would panic on the `expect` rather than measure. It does not -- and
// that, the observable outcome, is the whole of what is claimed here.
//
// That was an argument, and an argument is what a reviewer had to disbelieve.
// This is the measurement.
//
// **It pins less than two earlier versions of this comment claimed.** The first
// said it pinned "touches no filesystem"; the second said it showed the volume
// was "never consulted". Neither follows. A black-box success is equally
// compatible with a consultation whose failure is ignored, so nothing here
// reaches the mechanism -- which is the overreach the owning crate's `D-18`
// exists to remove, committed twice in the comment describing it.
//
// What this pins is the observable outcome, and it is exactly what the probe
// needs: the absence of a volume does not make preparation fail.

#[test]
fn preparing_a_path_needs_no_volume_behind_its_drive_letter() {
    // A bitmask, not 23 `Path::exists()` calls. Probing each root touches real
    // devices: an offline mapped network drive makes `exists()` block until the
    // redirector times out, so the original form could stall this test for
    // minutes on exactly the CI machine most likely to have one.
    // `GetLogicalDrives` answers from a snapshot without going near a device,
    // and answers the sharper question too -- a letter absent from the mask has
    // no volume, where `exists()` also returns false for a drive that is
    // present but has no media.
    //
    // SAFETY: no preconditions.
    let used = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };

    // `GetLogicalDrives` returns 0 on failure, which is also a perfectly valid
    // mask meaning "no drives at all" -- so an unchecked zero would be read as
    // "every letter is free", and the search would pick a letter that may well
    // be mounted. The test would then prepare a path on a REAL volume and pass,
    // proving nothing, which is worse than failing.
    //
    // That this test in particular could go vacuous is the sharp edge: it is
    // the one pinning the claim that no volume is needed. It is also a plain
    // instance of the workspace standard -- a failable call has its failure
    // handled -- introduced by the switch away from `Path::exists()`, which
    // could not fail this way.
    assert!(
        used != 0,
        "GetLogicalDrives failed: {}",
        std::io::Error::last_os_error()
    );

    // The whole alphabet. The mask already excludes anything mounted, so there
    // is no letter worth reserving by hand: starting at `D` only narrowed the
    // search on a machine with many mapped drives, for no benefit, since `C`
    // being in use is exactly what the mask reports.
    // Fails rather than skips when no letter is free, which is deliberate and
    // has been raised in review, so the reasoning is recorded here.
    //
    // libtest has no runtime skip: a test that "skips" is a test that PASSES.
    // This is the test pinning the claim that `prepare` needs no volume, so a
    // pass that established nothing is the one outcome worth avoiding -- the
    // same vacuous-green hazard as the unchecked `GetLogicalDrives` above, which
    // is what made the explicit check necessary in the first place.
    //
    // It also matches how this crate already handles the identical condition:
    // `impersonation_changes_which_device_map_a_drive_letter_resolves_in` and
    // its sibling both `panic!("no free drive letter on this host, so the probe
    // cannot run")`, and they search only `H..=Z`. This search covers all 26, so
    // it fails strictly less often than sites that already chose to fail.
    //
    // The condition needs every letter mounted including `A` and `B`, which are
    // floppy-era and essentially never assigned. A host in that state is worth
    // hearing about loudly, and the message says plainly that it is the
    // environment rather than the code.
    let absent = (b'A'..=b'Z')
        .find(|&byte| used & (1 << u32::from(byte - b'A')) == 0)
        .map(char::from)
        .expect(
            "every drive letter A-Z is mounted on this host, so no unmounted \
             letter exists to test against -- an environment limitation, not a \
             failure of the behaviour under test",
        );

    let text = format!(r"{absent}:\{}\file.txt", vec!["directory"; 24].join("\\"));
    let path = wtf_string::Wtf16String::from(text.as_str());

    assert!(
        windows_namespace_request_sys::prepare(&path).is_ok(),
        "preparing {text} must succeed with no {absent}: volume mounted. That \
         outcome is the whole claim -- this says nothing about whether a device \
         is consulted, because a black-box success cannot -- and it is what \
         `request_cost` depends on for its hard-coded long-path sample"
    );
}
