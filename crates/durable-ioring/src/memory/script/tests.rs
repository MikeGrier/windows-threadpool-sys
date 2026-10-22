// Copyright (c) 2026 Mike Grier
//! The fault script (DI-11.1): what each fault does to the operation, the target and the completion,
//! when a step fires, and what a script refuses to mean.

use std::num::NonZeroU32;

use super::{Fault, FaultScript, Fired, ScriptOp, When};
use crate::backend::{
    Completion, IoBackend, RefusalCause, RegisteredRegion, Submission, SubmissionData,
    SubmissionOp, SubmissionToken,
};
use crate::memory::{MEMORY, MemoryBackend, MemoryTarget, scripted_fault};
use crate::types::{IoFault, IoTargetKey, WriteCaching};

const KEY: IoTargetKey = IoTargetKey(7);
const OTHER: IoTargetKey = IoTargetKey(8);

fn scripted(script: FaultScript) -> MemoryBackend {
    scripted_queues(script, 8, 16)
}

fn scripted_queues(script: FaultScript, submissions: u32, completions: u32) -> MemoryBackend {
    let mut backend = MemoryBackend::new(
        NonZeroU32::new(submissions).expect("non-zero"),
        NonZeroU32::new(completions).expect("non-zero"),
    )
    .with_script(script);
    backend.attach(KEY, MemoryTarget::new()).expect("a new key");
    backend
        .attach(OTHER, MemoryTarget::new())
        .expect("a new key");
    backend
}

fn write_to(target: IoTargetKey, token: u64, offset: u64, bytes: &[u8]) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target,
        op: SubmissionOp::Write {
            offset,
            data: SubmissionData::Owned(bytes.to_vec()),
            caching: WriteCaching::Cached,
        },
    }
}

fn write(token: u64, offset: u64, bytes: &[u8]) -> Submission<Vec<u8>> {
    write_to(KEY, token, offset, bytes)
}

fn read(token: u64, offset: u64, length: usize) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target: KEY,
        op: SubmissionOp::Read {
            offset,
            data: SubmissionData::Owned(vec![0xEE; length]),
        },
    }
}

fn commit_to(target: IoTargetKey, token: u64) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target,
        op: SubmissionOp::Commit,
    }
}

fn commit(token: u64) -> Submission<Vec<u8>> {
    commit_to(KEY, token)
}

fn push(backend: &mut MemoryBackend, submissions: Vec<Submission<Vec<u8>>>) {
    for submission in submissions {
        backend.push(submission).expect("room");
    }
}

fn drain(backend: &mut MemoryBackend) -> Vec<Completion<Vec<u8>>> {
    std::iter::from_fn(|| backend.pop().expect("no script fails the pop here")).collect()
}

/// Push each, ring the doorbell, and pop everything ready.
fn run(
    backend: &mut MemoryBackend,
    submissions: Vec<Submission<Vec<u8>>>,
) -> Vec<Completion<Vec<u8>>> {
    push(backend, submissions);
    backend.submit().expect("the doorbell is not scripted");
    drain(backend)
}

fn tokens(completions: &[Completion<Vec<u8>>]) -> Vec<u64> {
    completions.iter().map(|c| c.token.0).collect()
}

fn contents(backend: &MemoryBackend) -> &[u8] {
    backend.target(KEY).expect("attached").contents()
}

fn durable(backend: &MemoryBackend) -> &[u8] {
    backend.target(KEY).expect("attached").durable_contents()
}

fn disk_full() -> IoFault {
    scripted_fault("the disk is full")
}

/// The fault `completion` ended with, which must be one.
fn failed(completion: &Completion<Vec<u8>>) -> &IoFault {
    completion
        .result
        .as_ref()
        .expect_err("the completion failed")
}

fn step(when: When, fault: Fault) -> FaultScript {
    FaultScript::new().on(when, fault)
}

const WRITES: When = When::any().op(ScriptOp::Write);
const COMMITS: When = When::any().op(ScriptOp::Commit);

#[test]
fn a_script_with_no_steps_changes_nothing() {
    let mut backend = scripted(FaultScript::new());
    assert!(FaultScript::new().is_empty());
    let done = run(
        &mut backend,
        vec![write(1, 0, b"ab"), commit(2), read(3, 0, 2)],
    );
    assert_eq!(tokens(&done), [1, 2, 3]);
    assert!(done.iter().all(|c| c.result.is_ok()));
    assert_eq!(durable(&backend), b"ab");
    assert!(backend.fired().is_empty());
}

#[test]
fn a_failed_write_leaves_the_target_as_it_was_and_hands_its_buffer_back() {
    let fault = disk_full();
    let mut backend = scripted(step(WRITES.nth(2), Fault::Fail(fault.clone())));
    let done = run(
        &mut backend,
        vec![write(1, 0, b"aa"), write(2, 2, b"bb"), write(3, 4, b"cc")],
    );
    assert_eq!(done[0].result, Ok(2));
    assert_eq!(failed(&done[1]), &fault);
    assert_eq!(failed(&done[1]).source, MEMORY);
    assert_eq!(done[1].buffer.as_deref(), Some(b"bb".as_slice()));
    assert_eq!(done[2].result, Ok(2));
    assert_eq!(
        contents(&backend),
        b"aa\0\0cc",
        "the second write never arrived"
    );
}

#[test]
fn a_failed_commit_makes_nothing_durable() {
    let mut backend = scripted(step(COMMITS, Fault::Fail(disk_full())));
    let done = run(&mut backend, vec![write(1, 0, b"lost"), commit(2)]);
    assert_eq!(done[0].result, Ok(4));
    assert!(done[1].result.is_err());
    assert!(done[1].buffer.is_none());
    assert_eq!(contents(&backend), b"lost");
    assert!(durable(&backend).is_empty(), "the flush failed");
}

#[test]
fn a_later_commit_after_a_failed_one_is_not_made_to_fail_by_it() {
    let mut backend = scripted(step(COMMITS.nth(1), Fault::Fail(disk_full())));
    let done = run(&mut backend, vec![write(1, 0, b"x"), commit(2), commit(3)]);
    assert!(done[1].result.is_err());
    assert_eq!(done[2].result, Ok(0));
    assert_eq!(durable(&backend), b"x", "the second commit succeeded");
}

#[test]
fn a_write_failed_after_applying_is_in_the_target_and_reported_failed() {
    let mut backend = scripted(step(WRITES, Fault::FailAfterApplying(disk_full())));
    let done = run(&mut backend, vec![write(1, 0, b"here")]);
    assert!(done[0].result.is_err());
    assert_eq!(done[0].buffer.as_deref(), Some(b"here".as_slice()));
    assert_eq!(contents(&backend), b"here");
}

#[test]
fn a_commit_failed_after_applying_is_durable_and_reported_failed() {
    let mut backend = scripted(step(COMMITS, Fault::FailAfterApplying(disk_full())));
    let done = run(&mut backend, vec![write(1, 0, b"kept"), commit(2)]);
    assert!(done[1].result.is_err());
    assert_eq!(durable(&backend), b"kept");
}

#[test]
fn a_read_failed_either_way_is_a_failed_read() {
    for fault in [
        Fault::Fail(disk_full()),
        Fault::FailAfterApplying(disk_full()),
    ] {
        let mut backend = scripted(step(When::any().op(ScriptOp::Read), fault));
        let done = run(&mut backend, vec![write(1, 0, b"abc"), read(2, 0, 3)]);
        assert_eq!(done[0].result, Ok(3));
        assert!(done[1].result.is_err());
        assert_eq!(done[1].buffer.as_ref().map(Vec::len), Some(3));
    }
}

#[test]
fn a_short_write_applies_only_the_count_it_reports() {
    let mut backend = scripted(step(WRITES, Fault::Short(2)));
    let done = run(&mut backend, vec![write(1, 0, b"abcd")]);
    assert_eq!(done[0].result, Ok(2));
    assert_eq!(done[0].buffer.as_deref(), Some(b"abcd".as_slice()));
    assert_eq!(contents(&backend), b"ab");
}

#[test]
fn a_short_read_transfers_only_the_count_it_reports() {
    let mut backend = scripted(step(When::any().op(ScriptOp::Read), Fault::Short(2)));
    let done = run(&mut backend, vec![write(1, 0, b"abcd"), read(2, 0, 4)]);
    assert_eq!(done[1].result, Ok(2));
    assert_eq!(
        done[1].buffer.as_deref(),
        Some([b'a', b'b', 0xEE, 0xEE].as_slice()),
        "the rest of the buffer is untouched"
    );
}

#[test]
fn a_short_count_at_or_beyond_the_length_is_the_whole_transfer() {
    for count in [4, 5, u32::MAX] {
        let mut backend = scripted(step(When::any(), Fault::Short(count)));
        let done = run(&mut backend, vec![write(1, 0, b"abcd"), read(2, 0, 4)]);
        assert_eq!(done[0].result, Ok(4), "count {count}");
        assert_eq!(done[1].result, Ok(4), "count {count}");
        assert_eq!(contents(&backend), b"abcd");
    }
}

#[test]
fn a_short_count_of_zero_transfers_nothing_and_succeeds() {
    let mut backend = scripted(step(WRITES, Fault::Short(0)));
    let done = run(&mut backend, vec![write(1, 0, b"abcd")]);
    assert_eq!(done[0].result, Ok(0));
    assert!(contents(&backend).is_empty());
}

#[test]
fn a_short_count_leaves_commits_whole() {
    // A step naming no operation applies the count where there is one.
    let mut backend = scripted(step(When::any(), Fault::Short(1)));
    let done = run(&mut backend, vec![write(1, 0, b"ab"), commit(2)]);
    assert_eq!(done[0].result, Ok(1));
    assert_eq!(done[1].result, Ok(0));
    assert_eq!(durable(&backend), b"a");
}

#[test]
fn a_short_registered_write_applies_only_the_count_it_reports() {
    let mut backend =
        scripted(step(WRITES, Fault::Short(3))).with_registered(vec![b"wxyz".to_vec()]);
    let region = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 4,
    };
    let submission = Submission {
        token: SubmissionToken(1),
        target: KEY,
        op: SubmissionOp::Write {
            offset: 0,
            data: SubmissionData::Registered(region),
            caching: WriteCaching::Cached,
        },
    };
    let done = run(&mut backend, vec![submission]);
    assert_eq!(done[0].result, Ok(3));
    assert_eq!(contents(&backend), b"wxy");
}

#[test]
fn a_held_completion_is_withheld_until_released_though_its_operation_ran() {
    let mut backend = scripted(step(WRITES.nth(1), Fault::Hold));
    let done = run(&mut backend, vec![write(1, 0, b"ab"), write(2, 2, b"cd")]);
    assert_eq!(tokens(&done), [2]);
    assert_eq!(backend.held(), [SubmissionToken(1)]);
    assert_eq!(
        contents(&backend),
        b"abcd",
        "it ran; only its completion waits"
    );
    assert!(backend.release(SubmissionToken(1)));
    let late = drain(&mut backend);
    assert_eq!(tokens(&late), [1]);
    assert_eq!(late[0].result, Ok(2));
    assert!(backend.held().is_empty());
}

#[test]
fn a_held_completion_is_released_once() {
    let mut backend = scripted(step(WRITES, Fault::Hold));
    run(&mut backend, vec![write(1, 0, b"a")]);
    assert!(
        !backend.release(SubmissionToken(99)),
        "nothing held under it"
    );
    assert!(backend.release(SubmissionToken(1)));
    assert!(!backend.release(SubmissionToken(1)), "it is no longer held");
    assert_eq!(tokens(&drain(&mut backend)), [1]);
}

#[test]
fn completions_released_in_a_chosen_order_come_out_in_it() {
    let mut backend = scripted(step(WRITES, Fault::Hold));
    run(
        &mut backend,
        vec![write(1, 0, b"a"), write(2, 1, b"b"), write(3, 2, b"c")],
    );
    assert_eq!(
        backend.held(),
        [SubmissionToken(1), SubmissionToken(2), SubmissionToken(3)]
    );
    for token in [3, 1, 2] {
        assert!(backend.release(SubmissionToken(token)));
    }
    assert_eq!(tokens(&drain(&mut backend)), [3, 1, 2], "out of order");
}

#[test]
fn releasing_everything_releases_in_the_order_held() {
    let mut backend = scripted(step(WRITES, Fault::Hold));
    run(&mut backend, vec![write(5, 0, b"a"), write(4, 1, b"b")]);
    assert_eq!(backend.release_all(), 2);
    assert_eq!(backend.release_all(), 0, "nothing left to release");
    assert_eq!(tokens(&drain(&mut backend)), [5, 4]);
}

#[test]
fn a_released_completion_follows_those_already_ready() {
    let mut backend = scripted(step(WRITES.nth(1), Fault::Hold));
    push(&mut backend, vec![write(1, 0, b"a"), write(2, 1, b"b")]);
    backend.submit().expect("doorbell");
    assert!(backend.release(SubmissionToken(1)));
    assert_eq!(tokens(&drain(&mut backend)), [2, 1]);
}

#[test]
fn a_held_commit_has_made_its_bytes_durable_before_it_completes() {
    let mut backend = scripted(step(COMMITS, Fault::Hold));
    let done = run(&mut backend, vec![write(1, 0, b"kept"), commit(2)]);
    assert_eq!(tokens(&done), [1]);
    assert_eq!(
        durable(&backend),
        b"kept",
        "it ran; only its completion waits"
    );
    backend.release_all();
    assert_eq!(drain(&mut backend)[0].result, Ok(0));
}

#[test]
fn a_late_completion_is_released_by_the_doorbell_that_many_doorbells_after() {
    let mut backend = scripted(step(WRITES.nth(1), Fault::Late(2)));
    push(&mut backend, vec![write(1, 0, b"a")]);
    backend.submit().expect("doorbell 1");
    assert!(drain(&mut backend).is_empty(), "executed, but late");
    backend.submit().expect("doorbell 2");
    assert!(drain(&mut backend).is_empty(), "still one doorbell short");
    backend.submit().expect("doorbell 3");
    assert_eq!(tokens(&drain(&mut backend)), [1]);
    assert!(backend.held().is_empty());
}

#[test]
fn a_late_completion_arrives_after_the_ones_executed_behind_it() {
    let mut backend = scripted(step(WRITES.nth(1), Fault::Late(1)));
    push(&mut backend, vec![write(1, 0, b"a"), write(2, 1, b"b")]);
    backend.submit().expect("doorbell 1");
    assert_eq!(tokens(&drain(&mut backend)), [2]);
    backend.submit().expect("doorbell 2");
    assert_eq!(tokens(&drain(&mut backend)), [1], "late, so out of order");
}

#[test]
fn a_late_completion_released_by_the_test_first_is_not_released_again() {
    let mut backend = scripted(step(WRITES, Fault::Late(2)));
    run(&mut backend, vec![write(1, 0, b"a")]);
    assert!(
        backend.release(SubmissionToken(1)),
        "a test may release it early"
    );
    assert_eq!(tokens(&drain(&mut backend)), [1]);
    backend.submit().expect("doorbell");
    backend.submit().expect("doorbell");
    assert!(drain(&mut backend).is_empty());
}

#[test]
fn a_late_of_zero_is_an_ordinary_completion() {
    let mut backend = scripted(step(WRITES, Fault::Late(0)));
    let done = run(&mut backend, vec![write(1, 0, b"a")]);
    assert_eq!(tokens(&done), [1]);
    assert!(backend.held().is_empty());
}

#[test]
fn a_scripted_refusal_hands_the_submission_back_and_queues_nothing() {
    let mut backend = scripted(step(
        When::any().nth(1),
        Fault::Refuse(RefusalCause::SubmitNeeded),
    ));
    let refused = backend.push(write(1, 0, b"kept")).expect_err("refused");
    assert_eq!(refused.cause, RefusalCause::SubmitNeeded);
    assert_eq!(refused.submission.token, SubmissionToken(1));
    assert!(matches!(
        refused.submission.op,
        SubmissionOp::Write { data: SubmissionData::Owned(ref bytes), .. } if bytes == b"kept"
    ));
    assert_eq!(backend.queued(), 0);
    backend
        .push(refused.submission)
        .expect("only the first is refused");
    assert_eq!(backend.queued(), 1);
}

#[test]
fn a_scripted_refusal_may_say_any_cause() {
    for cause in [
        RefusalCause::DrainNeeded,
        RefusalCause::SubmitAndDrainNeeded,
        RefusalCause::Busy,
        RefusalCause::Unsupported,
        RefusalCause::Unusable(disk_full()),
        RefusalCause::Unknown(disk_full()),
    ] {
        let mut backend = scripted(step(When::any(), Fault::Refuse(cause.clone())));
        let refused = backend.push(commit(1)).expect_err("refused");
        assert_eq!(refused.cause, cause);
    }
}

#[test]
fn a_real_refusal_is_not_masked_and_is_not_counted() {
    let mut backend = scripted(step(When::any().nth(1), Fault::Refuse(RefusalCause::Busy)));
    let unknown = Submission {
        token: SubmissionToken(1),
        target: IoTargetKey(99),
        op: SubmissionOp::Commit,
    };
    assert_eq!(
        backend.push(unknown).expect_err("refused").cause,
        RefusalCause::UnknownTarget,
        "the real reason, not the script's"
    );
    assert_eq!(
        backend
            .push(commit(2))
            .expect_err("the first admitted push")
            .cause,
        RefusalCause::Busy,
        "the refused-for-real push did not use the script's one"
    );
    backend.push(commit(3)).expect("the second admitted push");
}

#[test]
fn a_scripted_full_queue_is_refused_though_the_queue_is_not_full() {
    let mut backend = scripted(step(
        WRITES.from(2),
        Fault::Refuse(RefusalCause::DrainNeeded),
    ));
    backend
        .push(write(1, 0, b"a"))
        .expect("the first is admitted");
    let refused = backend.push(write(2, 1, b"b")).expect_err("scripted full");
    assert_eq!(refused.cause, RefusalCause::DrainNeeded);
    assert!(refused.cause.needs_drain() && !refused.cause.needs_submit());
    backend
        .push(commit(3))
        .expect("a commit is not matched by a step naming writes");
}

#[test]
fn a_refused_doorbell_issues_nothing_and_leaves_the_queue_as_it_was() {
    let mut backend = scripted(step(When::any(), Fault::RefuseDoorbell(RefusalCause::Busy)));
    push(&mut backend, vec![write(1, 0, b"a")]);
    assert_eq!(backend.submit(), Err(RefusalCause::Busy));
    assert_eq!(backend.queued(), 1);
    assert!(contents(&backend).is_empty(), "nothing was issued");
}

#[test]
fn a_refused_doorbell_on_one_occasion_clears_on_the_next() {
    let mut backend = scripted(step(
        When::any().nth(1),
        Fault::RefuseDoorbell(RefusalCause::Busy),
    ));
    push(&mut backend, vec![write(1, 0, b"a")]);
    assert!(backend.submit().is_err());
    backend
        .submit()
        .expect("the second doorbell is not scripted");
    assert_eq!(tokens(&drain(&mut backend)), [1]);
    assert_eq!(contents(&backend), b"a");
}

#[test]
fn a_failed_pop_takes_nothing_and_the_completion_is_still_ready() {
    let fault = disk_full();
    let mut backend = scripted(step(When::any().nth(1), Fault::FailPop(fault.clone())));
    push(&mut backend, vec![write(1, 0, b"a")]);
    backend.submit().expect("doorbell");
    assert_eq!(backend.pop().expect_err("scripted"), fault);
    assert_eq!(backend.completed(), 1, "it was not taken");
    assert_eq!(tokens(&drain(&mut backend)), [1]);
}

#[test]
fn a_held_completion_still_holds_its_place_in_the_completion_queue() {
    let mut backend = scripted_queues(step(WRITES.nth(1), Fault::Hold), 8, 2);
    backend.push(write(1, 0, b"a")).expect("room");
    backend.submit().expect("doorbell");
    backend
        .push(write(2, 1, b"b"))
        .expect("room: one held, one queued");
    assert_eq!(
        backend.push(write(3, 2, b"c")).expect_err("no room").cause,
        RefusalCause::DrainNeeded,
        "the held completion is outstanding, so a completion would not fit"
    );
}

#[test]
fn registered_buffers_cannot_be_replaced_while_a_completion_is_held() {
    let mut backend = scripted(step(WRITES, Fault::Hold));
    run(&mut backend, vec![write(1, 0, b"a")]);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        backend.with_registered(vec![vec![0; 4]])
    }));
    assert!(outcome.is_err(), "a held operation may name a region");
}

#[test]
fn a_step_matches_by_kind_and_by_target() {
    let mut backend = scripted(step(
        When::any().op(ScriptOp::Write).target(OTHER),
        Fault::Fail(disk_full()),
    ));
    let done = run(
        &mut backend,
        vec![
            write_to(KEY, 1, 0, b"a"),
            write_to(OTHER, 2, 0, b"b"),
            commit_to(OTHER, 3),
        ],
    );
    assert!(done[0].result.is_ok(), "another target");
    assert!(done[1].result.is_err(), "the target and the kind");
    assert!(done[2].result.is_ok(), "the target, but a commit");
}

#[test]
fn a_steps_count_is_of_its_own_matches() {
    // The second write to OTHER, though it is the third write overall.
    let mut backend = scripted(step(WRITES.target(OTHER).nth(2), Fault::Fail(disk_full())));
    let done = run(
        &mut backend,
        vec![
            write_to(OTHER, 1, 0, b"a"),
            write_to(KEY, 2, 0, b"b"),
            write_to(OTHER, 3, 1, b"c"),
        ],
    );
    assert_eq!(
        done.iter().map(|c| c.result.is_ok()).collect::<Vec<_>>(),
        [true, true, false]
    );
}

#[test]
fn from_fires_on_the_nth_and_every_one_after() {
    let mut backend = scripted(step(WRITES.from(2), Fault::Fail(disk_full())));
    let done = run(
        &mut backend,
        vec![write(1, 0, b"a"), write(2, 1, b"b"), write(3, 2, b"c")],
    );
    assert_eq!(
        done.iter().map(|c| c.result.is_ok()).collect::<Vec<_>>(),
        [true, false, false]
    );
}

#[test]
fn with_no_occurrence_a_step_fires_every_time() {
    let mut backend = scripted(step(COMMITS, Fault::Fail(disk_full())));
    let done = run(&mut backend, vec![commit(1), commit(2), commit(3)]);
    assert!(done.iter().all(|c| c.result.is_err()));
}

#[test]
fn every_matching_step_counts_but_only_the_earliest_that_fires_applies() {
    let first = scripted_fault("first");
    let second = scripted_fault("second");
    let script = FaultScript::new()
        .on(WRITES.nth(2), Fault::Fail(first.clone()))
        .on(WRITES, Fault::Fail(second.clone()));
    let mut backend = scripted(script);
    let done = run(
        &mut backend,
        vec![write(1, 0, b"a"), write(2, 1, b"b"), write(3, 2, b"c")],
    );
    assert_eq!(
        failed(&done[0]),
        &second,
        "only the second step fires for the first write"
    );
    assert_eq!(
        failed(&done[1]),
        &first,
        "both fire for the second, and the earlier wins"
    );
    assert_eq!(failed(&done[2]), &second);
}

#[test]
fn a_step_that_lost_to_an_earlier_one_still_counted_the_occurrence() {
    let script = FaultScript::new()
        .on(WRITES.nth(1), Fault::Hold)
        .on(WRITES.nth(2), Fault::Fail(disk_full()));
    let mut backend = scripted(script);
    let done = run(&mut backend, vec![write(1, 0, b"a"), write(2, 1, b"b")]);
    assert_eq!(tokens(&done), [2]);
    assert!(
        done[0].result.is_err(),
        "the second step counted the first write too"
    );
}

#[test]
fn what_fired_is_recorded_in_order_with_what_it_fired_for() {
    let script = FaultScript::new()
        .on(COMMITS, Fault::Fail(disk_full()))
        .on(WRITES.target(OTHER), Fault::Hold);
    let mut backend = scripted(script);
    run(
        &mut backend,
        vec![write(1, 0, b"a"), write_to(OTHER, 2, 0, b"b"), commit(3)],
    );
    assert_eq!(
        backend.fired(),
        [
            Fired {
                step: 1,
                token: Some(SubmissionToken(2)),
                op: Some(ScriptOp::Write),
                target: Some(OTHER),
            },
            Fired {
                step: 0,
                token: Some(SubmissionToken(3)),
                op: Some(ScriptOp::Commit),
                target: Some(KEY),
            },
        ]
    );
}

#[test]
fn a_doorbell_or_a_pop_that_fired_is_recorded_without_a_submission() {
    let script = FaultScript::new().on(When::any().nth(1), Fault::FailPop(disk_full()));
    let mut backend = scripted(script);
    assert!(backend.pop().is_err());
    assert_eq!(
        backend.fired(),
        [Fired {
            step: 0,
            token: None,
            op: None,
            target: None,
        }]
    );
}

#[test]
fn a_script_is_plain_data_and_each_backend_counts_it_from_the_start() {
    let script = step(WRITES.nth(1), Fault::Fail(disk_full()));
    assert_eq!(script.len(), 1);
    for _ in 0..2 {
        let mut backend = scripted(script.clone());
        let done = run(&mut backend, vec![write(1, 0, b"a"), write(2, 1, b"b")]);
        assert!(done[0].result.is_err());
        assert!(done[1].result.is_ok());
    }
}

#[test]
fn releasing_into_an_empty_queue_raises_the_notification_once() {
    let mut backend = scripted(step(WRITES, Fault::Hold));
    let notification = backend.notification().expect("a notification");
    run(&mut backend, vec![write(1, 0, b"a"), write(2, 1, b"b")]);
    assert_eq!(notification.raised(), 0, "nothing was ready");
    backend.release_all();
    assert_eq!(notification.raised(), 1, "none to some, once for both");
    backend.release(SubmissionToken(1));
    assert_eq!(notification.raised(), 1, "nothing was held to release");
}

#[test]
fn a_single_release_into_an_empty_queue_raises_the_notification() {
    let mut backend = scripted(step(WRITES, Fault::Hold));
    let notification = backend.notification().expect("a notification");
    run(&mut backend, vec![write(1, 0, b"a"), write(2, 1, b"b")]);
    backend.release(SubmissionToken(2));
    assert_eq!(notification.raised(), 1);
    backend.release(SubmissionToken(1));
    assert_eq!(notification.raised(), 1, "the queue was not empty");
}

#[test]
fn a_late_release_into_an_empty_queue_raises_the_notification() {
    let mut backend = scripted(step(WRITES, Fault::Late(1)));
    let notification = backend.notification().expect("a notification");
    push(&mut backend, vec![write(1, 0, b"a")]);
    backend.submit().expect("doorbell 1");
    assert_eq!(notification.raised(), 0);
    backend.submit().expect("doorbell 2");
    assert_eq!(notification.raised(), 1);
}

#[test]
#[should_panic(expected = "a commit has no count")]
fn a_short_count_on_commits_is_a_scripts_mistake() {
    let _ = step(COMMITS, Fault::Short(1));
}

#[test]
#[should_panic(expected = "no operation or target to name")]
fn a_refused_doorbell_naming_an_operation_is_a_scripts_mistake() {
    let _ = step(
        When::any().op(ScriptOp::Write),
        Fault::RefuseDoorbell(RefusalCause::Busy),
    );
}

#[test]
#[should_panic(expected = "no operation or target to name")]
fn a_failed_pop_naming_a_target_is_a_scripts_mistake() {
    let _ = step(When::any().target(KEY), Fault::FailPop(disk_full()));
}

#[test]
#[should_panic(expected = "counted from one")]
fn the_zeroth_occurrence_is_a_scripts_mistake() {
    let _ = When::any().nth(0);
}

#[test]
#[should_panic(expected = "counted from one")]
fn from_the_zeroth_occurrence_is_a_scripts_mistake() {
    let _ = When::any().from(0);
}
