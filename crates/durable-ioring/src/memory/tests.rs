// Copyright (c) 2026 Mike Grier
//! The in-memory backend: execution, refusals, and its notification.

use std::num::NonZeroU32;
use std::thread;
use std::time::Duration;

use super::{MEMORY, MemoryBackend, MemoryTarget};
use crate::backend::{
    Completion, IoBackend, RefusalCause, RegisteredRegion, Submission, SubmissionData,
    SubmissionOp, SubmissionToken,
};
use crate::io_target::{Geometry, IoTarget};
use crate::types::{IoTargetKey, WriteCaching};

const KEY: IoTargetKey = IoTargetKey(7);

fn queues(submissions: u32, completions: u32) -> MemoryBackend {
    MemoryBackend::new(
        NonZeroU32::new(submissions).expect("non-zero"),
        NonZeroU32::new(completions).expect("non-zero"),
    )
}

fn backend() -> MemoryBackend {
    let mut backend = queues(8, 16);
    backend.attach(KEY, MemoryTarget::new()).expect("a new key");
    backend
}

fn write(token: u64, offset: u64, bytes: &[u8]) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target: KEY,
        op: SubmissionOp::Write {
            offset,
            data: SubmissionData::Owned(bytes.to_vec()),
            caching: WriteCaching::Cached,
        },
    }
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

fn commit(token: u64) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target: KEY,
        op: SubmissionOp::Commit,
    }
}

fn registered(token: u64, write: bool, region: RegisteredRegion) -> Submission<Vec<u8>> {
    let data = SubmissionData::Registered(region);
    Submission {
        token: SubmissionToken(token),
        target: KEY,
        op: if write {
            SubmissionOp::Write {
                offset: 0,
                data,
                caching: WriteCaching::Cached,
            }
        } else {
            SubmissionOp::Read { offset: 0, data }
        },
    }
}

/// Push each, submit, and pop every completion.
fn run(
    backend: &mut MemoryBackend,
    submissions: Vec<Submission<Vec<u8>>>,
) -> Vec<Completion<Vec<u8>>> {
    for submission in submissions {
        backend.push(submission).expect("room");
    }
    backend
        .submit()
        .expect("the memory backend's submit never fails");
    drain(backend)
}

fn drain(backend: &mut MemoryBackend) -> Vec<Completion<Vec<u8>>> {
    let mut completions = Vec::new();
    while let Some(completion) = backend.pop().expect("the memory backend's pop never fails") {
        completions.push(completion);
    }
    completions
}

fn contents(backend: &MemoryBackend) -> &[u8] {
    backend.target(KEY).expect("attached").contents()
}

fn durable(backend: &MemoryBackend) -> &[u8] {
    backend.target(KEY).expect("attached").durable_contents()
}

fn refusal(backend: &mut MemoryBackend, submission: Submission<Vec<u8>>) -> RefusalCause {
    backend.push(submission).expect_err("refused").cause
}

#[test]
fn a_write_is_read_back() {
    let mut backend = backend();
    let done = run(&mut backend, vec![write(1, 0, b"hello"), read(2, 0, 5)]);
    assert_eq!(done[0].result, Ok(5));
    assert_eq!(done[1].result, Ok(5));
    assert_eq!(done[1].buffer.as_deref(), Some(b"hello".as_slice()));
}

#[test]
fn completions_come_in_push_order_with_their_tokens_and_buffers() {
    let mut backend = backend();
    let done = run(
        &mut backend,
        vec![
            write(30, 0, b"a"),
            write(10, 1, b"bc"),
            commit(20),
            read(40, 0, 3),
        ],
    );
    let tokens: Vec<u64> = done.iter().map(|c| c.token.0).collect();
    assert_eq!(tokens, [30, 10, 20, 40]);
    assert_eq!(done[0].buffer.as_deref(), Some(b"a".as_slice()));
    assert_eq!(done[1].buffer.as_deref(), Some(b"bc".as_slice()));
    assert!(done[2].buffer.is_none(), "a commit carries no buffer");
    assert_eq!(done[2].result, Ok(0), "a commit transfers nothing");
    assert_eq!(done[3].buffer.as_deref(), Some(b"abc".as_slice()));
}

#[test]
fn nothing_runs_before_submit() {
    let mut backend = backend();
    backend.push(write(1, 0, b"x")).expect("room");
    assert_eq!(backend.queued(), 1);
    assert!(contents(&backend).is_empty());
    assert!(backend.pop().expect("pop").is_none());
    backend.submit().expect("submit");
    assert_eq!(backend.queued(), 0);
    assert_eq!(backend.completed(), 1);
    assert_eq!(contents(&backend), b"x");
}

#[test]
fn a_write_past_the_end_fills_the_gap_with_zeros() {
    let mut backend = backend();
    run(&mut backend, vec![write(1, 4, b"ab")]);
    assert_eq!(contents(&backend), [0, 0, 0, 0, b'a', b'b']);
}

#[test]
fn a_later_write_overwrites_an_earlier_one() {
    let mut backend = backend();
    run(&mut backend, vec![write(1, 0, b"aaaa"), write(2, 1, b"bb")]);
    assert_eq!(contents(&backend), b"abba");
}

#[test]
fn a_read_past_the_end_is_short() {
    let mut backend = backend();
    let done = run(
        &mut backend,
        vec![
            write(1, 0, b"abc"),
            read(2, 1, 8),
            read(3, 3, 4),
            read(4, 50, 4),
        ],
    );
    assert_eq!(done[1].result, Ok(2));
    assert_eq!(
        &done[1].buffer.as_deref().expect("owned")[..3],
        b"bc\xEE",
        "only the count is written"
    );
    assert_eq!(done[2].result, Ok(0), "a read at the end transfers nothing");
    assert_eq!(
        done[3].result,
        Ok(0),
        "a read beyond the end transfers nothing"
    );
}

#[test]
fn an_empty_write_transfers_nothing_and_changes_nothing() {
    let mut backend = backend();
    let done = run(&mut backend, vec![write(1, 0, b"")]);
    assert_eq!(done[0].result, Ok(0));
    assert!(contents(&backend).is_empty());
}

#[test]
fn a_commit_makes_what_was_written_durable_and_nothing_after() {
    let mut backend = backend();
    run(&mut backend, vec![write(1, 0, b"one")]);
    assert!(
        durable(&backend).is_empty(),
        "nothing is durable before a commit"
    );
    run(&mut backend, vec![commit(2)]);
    assert_eq!(durable(&backend), b"one");
    run(&mut backend, vec![write(3, 0, b"two")]);
    assert_eq!(
        durable(&backend),
        b"one",
        "a later write is not durable until committed"
    );
    assert_eq!(contents(&backend), b"two");
    run(&mut backend, vec![commit(4)]);
    assert_eq!(durable(&backend), b"two");
}

#[test]
fn a_full_submission_queue_needs_a_submit() {
    let mut backend = queues(2, 16);
    backend.attach(KEY, MemoryTarget::new()).expect("a new key");
    backend.push(write(1, 0, b"a")).expect("room");
    backend.push(write(2, 0, b"b")).expect("room");
    let refused = backend.push(write(3, 0, b"c")).expect_err("full");
    assert_eq!(refused.cause, RefusalCause::SubmitNeeded);
    backend.submit().expect("submit");
    backend
        .push(refused.submission)
        .expect("room after a submit");
}

#[test]
fn a_refused_submission_comes_back_unchanged() {
    let mut backend = queues(1, 16);
    backend.attach(KEY, MemoryTarget::new()).expect("a new key");
    backend.push(commit(1)).expect("room");
    let refused = backend.push(write(9, 12, b"kept")).expect_err("full");
    assert_eq!(refused.submission.token, SubmissionToken(9));
    assert_eq!(refused.submission.target, KEY);
    let SubmissionOp::Write {
        offset,
        data: SubmissionData::Owned(buffer),
        caching,
    } = refused.submission.op
    else {
        panic!("the write");
    };
    assert_eq!(
        (offset, buffer.as_slice(), caching),
        (12, b"kept".as_slice(), WriteCaching::Cached)
    );
}

#[test]
fn a_full_completion_queue_needs_a_drain() {
    let mut backend = queues(8, 2);
    backend.attach(KEY, MemoryTarget::new()).expect("a new key");
    backend.push(commit(1)).expect("room");
    backend.submit().expect("submit");
    backend
        .push(commit(2))
        .expect("one queued and one completed fill the queue");
    assert_eq!(refusal(&mut backend, commit(3)), RefusalCause::DrainNeeded);
    backend.submit().expect("submit");
    assert_eq!(
        refusal(&mut backend, commit(3)),
        RefusalCause::DrainNeeded,
        "a submit does not make room for a completion"
    );
    backend.pop().expect("pop").expect("a completion");
    backend.push(commit(3)).expect("room after a drain");
}

#[test]
fn both_queues_full_needs_both() {
    let mut backend = queues(1, 1);
    backend.attach(KEY, MemoryTarget::new()).expect("a new key");
    backend.push(commit(1)).expect("room");
    assert_eq!(
        refusal(&mut backend, commit(2)),
        RefusalCause::SubmitAndDrainNeeded
    );
}

#[test]
fn an_unattached_key_is_an_unknown_target() {
    let mut backend = backend();
    for op in [write(1, 0, b"x"), read(2, 0, 1), commit(3)] {
        let submission = Submission {
            target: IoTargetKey(99),
            ..op
        };
        assert_eq!(
            refusal(&mut backend, submission),
            RefusalCause::UnknownTarget
        );
    }
}

#[test]
fn an_unknown_target_is_named_before_a_full_queue() {
    let mut backend = queues(1, 1);
    backend.attach(KEY, MemoryTarget::new()).expect("a new key");
    backend.push(commit(1)).expect("room");
    let submission = Submission {
        target: IoTargetKey(99),
        ..commit(2)
    };
    assert_eq!(
        refusal(&mut backend, submission),
        RefusalCause::UnknownTarget
    );
}

#[test]
fn a_registered_region_without_registration_is_unsupported() {
    let mut backend = backend();
    assert_eq!(backend.registered_buffers(), 0);
    let region = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 1,
    };
    assert_eq!(
        refusal(&mut backend, registered(1, true, region)),
        RefusalCause::Unsupported
    );
}

#[test]
fn registered_regions_are_written_from_and_read_into() {
    let mut backend = backend().with_registered(vec![b"..source..".to_vec(), vec![0; 6]]);
    assert_eq!(backend.registered_buffers(), 2);
    let source = RegisteredRegion {
        index: 0,
        offset: 2,
        length: 6,
    };
    let sink = RegisteredRegion {
        index: 1,
        offset: 0,
        length: 6,
    };
    let done = run(
        &mut backend,
        vec![registered(1, true, source), registered(2, false, sink)],
    );
    assert_eq!(done[0].result, Ok(6));
    assert!(done[0].buffer.is_none());
    assert_eq!(done[1].result, Ok(6));
    assert_eq!(contents(&backend), b"source");
    assert_eq!(backend.registered(1), Some(b"source".as_slice()));
}

#[test]
fn a_region_outside_its_buffer_is_invalid() {
    let mut backend = backend().with_registered(vec![vec![0; 8]]);
    for (index, offset, length) in [(1, 0, 1), (0, 8, 1), (0, 4, 5), (0, u32::MAX, 2)] {
        let region = RegisteredRegion {
            index,
            offset,
            length,
        };
        assert_eq!(
            refusal(&mut backend, registered(1, true, region)),
            RefusalCause::Invalid,
            "{region:?}"
        );
    }
}

#[test]
fn the_targets_geometry_is_enforced() {
    let mut backend = queues(8, 16);
    let geometry = Geometry::new(512, 1024, 1).expect("powers of two");
    backend
        .attach(KEY, MemoryTarget::new().with_geometry(geometry))
        .expect("a new key");
    assert_eq!(
        refusal(&mut backend, write(1, 256, &[0; 1024])),
        RefusalCause::Invalid
    );
    assert_eq!(
        refusal(&mut backend, write(2, 512, &[0; 512])),
        RefusalCause::Invalid
    );
    assert_eq!(
        refusal(&mut backend, read(3, 1, 1024)),
        RefusalCause::Invalid
    );
    backend.push(write(4, 512, &[0; 1024])).expect("aligned");
    backend.push(commit(5)).expect("a commit has no geometry");
}

#[test]
fn a_buffers_address_is_checked_against_the_geometry() {
    let mut backend = queues(8, 16).with_registered(vec![vec![0; 8]]);
    let geometry = Geometry::new(1, 1, 2).expect("powers of two");
    backend
        .attach(KEY, MemoryTarget::new().with_geometry(geometry))
        .expect("a new key");
    // Of two addresses one apart, exactly one is even.
    let admitted = [0, 1]
        .into_iter()
        .filter(|&offset| {
            let region = RegisteredRegion {
                index: 0,
                offset,
                length: 2,
            };
            backend.push(registered(1, true, region)).is_ok()
        })
        .count();
    assert_eq!(admitted, 1);
}

#[test]
fn check_answers_what_push_does_whatever_the_queues_hold() {
    // Both directions, case by case: every submission `check` admits a push with room accepts, and
    // every one it refuses a push refuses for the same cause -- with the queues full as well, which
    // `check` does not look at but a push names after the cause `check` gives.
    let geometry = Geometry::new(512, 512, 1).expect("powers of two");
    let other = IoTargetKey(8);
    let region = |index, offset, length| RegisteredRegion {
        index,
        offset,
        length,
    };
    let at = |target, submission: Submission<Vec<u8>>| Submission {
        target,
        ..submission
    };
    let cases: Vec<(Submission<Vec<u8>>, Option<RefusalCause>)> = vec![
        (write(1, 0, b"abc"), None),
        (read(2, 0, 4), None),
        (commit(3), None),
        (registered(4, true, region(0, 0, 8)), None),
        (registered(5, false, region(0, 4, 4)), None),
        (at(other, write(6, 512, &[0; 512])), None),
        (
            at(IoTargetKey(99), write(7, 0, b"x")),
            Some(RefusalCause::UnknownTarget),
        ),
        (
            at(IoTargetKey(99), commit(8)),
            Some(RefusalCause::UnknownTarget),
        ),
        (
            registered(9, true, region(1, 0, 1)),
            Some(RefusalCause::Invalid),
        ),
        (
            registered(10, false, region(0, 4, 5)),
            Some(RefusalCause::Invalid),
        ),
        (
            at(other, write(11, 3, &[0; 512])),
            Some(RefusalCause::Invalid),
        ),
        (at(other, read(12, 512, 3)), Some(RefusalCause::Invalid)),
    ];
    for full in [false, true] {
        let mut backend = queues(1, 16).with_registered(vec![vec![0; 8]]);
        backend.attach(KEY, MemoryTarget::new()).expect("a new key");
        backend
            .attach(other, MemoryTarget::new().with_geometry(geometry))
            .expect("a new key");
        for (submission, cause) in &cases {
            assert_eq!(
                backend.check(submission.target, &submission.op).err(),
                *cause,
                "{submission:?}"
            );
        }
        if full {
            backend.push(commit(0)).expect("fills the one-entry queue");
        }
        for (submission, cause) in cases.iter().map(|(s, c)| (clone(s), c.clone())) {
            let pushed = backend.push(submission).err().map(|refused| refused.cause);
            let expected = match (cause, full) {
                (Some(cause), _) => Some(cause),
                (None, true) => Some(RefusalCause::SubmitNeeded),
                (None, false) => None,
            };
            assert_eq!(pushed, expected, "full: {full}");
            if pushed.is_none() {
                backend.submit().expect("never fails");
                drain(&mut backend);
            }
        }
    }
}

/// A copy of a submission, owned buffer included.
fn clone(submission: &Submission<Vec<u8>>) -> Submission<Vec<u8>> {
    let data = |data: &SubmissionData<Vec<u8>>| match data {
        SubmissionData::Owned(buffer) => SubmissionData::Owned(buffer.clone()),
        SubmissionData::Registered(region) => SubmissionData::Registered(*region),
    };
    Submission {
        token: submission.token,
        target: submission.target,
        op: match &submission.op {
            SubmissionOp::Write {
                offset,
                data: bytes,
                caching,
            } => SubmissionOp::Write {
                offset: *offset,
                data: data(bytes),
                caching: *caching,
            },
            SubmissionOp::Read {
                offset,
                data: bytes,
            } => SubmissionOp::Read {
                offset: *offset,
                data: data(bytes),
            },
            SubmissionOp::Commit => SubmissionOp::Commit,
        },
    }
}

#[test]
fn a_duplicate_key_hands_the_target_back() {
    let mut backend = backend();
    let target = MemoryTarget::new().with_geometry(Geometry::uniform(8).expect("a power of two"));
    let duplicate = backend.attach(KEY, target).expect_err("taken");
    assert_eq!(duplicate.key, KEY);
    assert_eq!(
        duplicate.target.geometry(),
        Geometry::uniform(8).expect("a power of two")
    );
}

#[test]
fn detach_hands_the_target_back_with_its_bytes() {
    let mut backend = backend();
    run(&mut backend, vec![write(1, 0, b"kept"), commit(2)]);
    let target = backend.detach(KEY).expect("attached");
    assert_eq!(target.contents(), b"kept");
    assert_eq!(target.durable_contents(), b"kept");
    assert!(backend.detach(KEY).is_none());
    assert!(backend.target(KEY).is_none());
}

#[test]
fn a_submission_for_a_detached_target_completes_with_a_fault_and_its_buffer() {
    let mut backend = backend();
    backend.push(write(1, 0, b"orphan")).expect("room");
    backend.push(commit(2)).expect("room");
    backend.detach(KEY).expect("attached");
    backend.submit().expect("submit");
    let done = drain(&mut backend);
    let fault = done[0].result.clone().expect_err("detached");
    assert_eq!(fault.source, MEMORY);
    assert_eq!(fault.code, None);
    assert_eq!(done[0].buffer.as_deref(), Some(b"orphan".as_slice()));
    assert!(done[1].result.is_err());
    assert!(done[1].buffer.is_none());
}

#[test]
fn the_notification_is_raised_only_when_completions_go_from_none_to_some() {
    let mut backend = backend();
    let notification = backend.notification().expect("a handle");
    backend.submit().expect("submit");
    assert_eq!(notification.raised(), 0, "nothing completed");
    backend.push(commit(1)).expect("room");
    backend.submit().expect("submit");
    assert_eq!(notification.raised(), 1);
    backend.push(commit(2)).expect("room");
    backend.submit().expect("submit");
    assert_eq!(notification.raised(), 1, "completions were already waiting");
    drain(&mut backend);
    backend.push(commit(3)).expect("room");
    backend.submit().expect("submit");
    assert_eq!(notification.raised(), 2);
}

#[test]
fn a_waiter_sees_a_raise_from_another_thread() {
    let mut backend = backend();
    let notification = backend.notification().expect("a handle");
    let waiter = thread::spawn(move || notification.wait_past(0, Duration::from_secs(10)));
    backend.push(commit(1)).expect("room");
    backend.submit().expect("submit");
    assert_eq!(waiter.join().expect("the waiter"), 1);
}

#[test]
fn a_wait_with_nothing_raised_times_out_with_the_count() {
    let backend = backend();
    let notification = backend.notification().expect("a handle");
    assert_eq!(notification.wait_past(0, Duration::from_millis(1)), 0);
}
