// Copyright (c) 2026 Mike Grier
//! The core driven over the in-memory backend: operations, completions, seals and commits,
//! refusals, the queue, and removing a target.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{CoreActions, DurableIoCore};
use crate::backend::{
    Completion, IoBackend, RefusalCause, RegisteredRegion, Submission, SubmissionData,
    SubmissionOp, SubmissionToken,
};
use crate::clock::StampClock;
use crate::diagnostics::NoDiagnostics;
use crate::ids::{DioringIds, OpId};
use crate::io_target::IoTarget;
use crate::memory::{MemoryBackend, MemoryTarget};
use crate::types::{
    DurabilityRequest, Entry, Epoch, EpochState, FailureCause, FlushDomain, IoTargetKey, OpKind,
    Outcome, PushError, PushRefusal, RemoveTargetError, SealError, WriteCaching,
};

const KEY: IoTargetKey = IoTargetKey(1);
const OTHER: IoTargetKey = IoTargetKey(2);

/// A clock that counts its readings, so the core's stamps are plain numbers.
#[derive(Clone, Default)]
struct Ticks(Arc<AtomicU64>);

impl StampClock for Ticks {
    type Instant = u64;

    fn now(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

type Core = DurableIoCore<u64, Vec<u8>, &'static str, Ticks>;
type V = DioringIds<u64, u64>;
type Pushed = Result<OpId, PushError<V, Vec<u8>, &'static str>>;

fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("non-zero")
}

/// A core and a backend, with the driver's part played here.
struct Rig {
    core: Core,
    backend: MemoryBackend,
}

impl Rig {
    fn new() -> Self {
        Self::with_queues(8, 16)
    }

    fn with_queues(submissions: u32, completions: u32) -> Self {
        let mut rig = Self {
            core: Core::new(Ticks::default(), Arc::new(NoDiagnostics)),
            backend: MemoryBackend::new(nz(submissions), nz(completions)),
        };
        rig.attach(KEY, MemoryTarget::new());
        rig
    }

    fn attach(&mut self, key: IoTargetKey, target: MemoryTarget) {
        self.core
            .add_target(key, target.describe())
            .expect("a new key");
        self.backend.attach(key, target).expect("a new key");
    }

    fn epoch(&self, id: u64) -> Epoch<V> {
        Epoch::new(self.core.default_lineage(), id)
    }

    fn write_to(&mut self, key: IoTargetKey, epoch: u64, offset: u64, bytes: &[u8]) -> Pushed {
        let epoch = self.epoch(epoch);
        let backend = &mut self.backend;
        self.core.write(
            key,
            offset,
            SubmissionData::Owned(bytes.to_vec()),
            epoch,
            "write",
            WriteCaching::Cached,
            |submission| backend.push(submission),
        )
    }

    fn write(&mut self, epoch: u64, offset: u64, bytes: &[u8]) -> Pushed {
        self.write_to(KEY, epoch, offset, bytes)
    }

    fn read(&mut self, offset: u64, length: usize) -> Pushed {
        let backend = &mut self.backend;
        self.core.read(
            KEY,
            offset,
            SubmissionData::Owned(vec![0; length]),
            "read",
            |submission| backend.push(submission),
        )
    }

    fn seal(&mut self, through: u64) -> DurabilityRequest<V> {
        let (answer, actions) = self
            .core
            .make_durable_through(self.epoch(through))
            .expect("the default lineage");
        self.act(actions);
        answer
    }

    /// Push what the core asks for, handing refusals back.
    fn act(&mut self, actions: CoreActions<Vec<u8>>) -> bool {
        for submission in actions.submit {
            if let Err(refused) = self.backend.push(submission) {
                self.core.refused(refused);
            }
        }
        actions.deliver
    }

    /// Submit and feed completions back until nothing more is queued or completed.
    fn run(&mut self) {
        loop {
            self.backend
                .submit()
                .expect("the memory backend's submit never fails");
            let mut any = false;
            while let Some(completion) = self.backend.pop().expect("never fails") {
                any = true;
                let actions = self.core.complete(completion);
                self.act(actions);
            }
            if !any && self.backend.queued() == 0 {
                return;
            }
        }
    }

    fn entries(&mut self) -> Vec<Entry<V, Vec<u8>, &'static str>> {
        std::iter::from_fn(|| self.core.pop()).collect()
    }

    fn target(&self) -> &MemoryTarget {
        self.backend.target(KEY).expect("attached")
    }
}

/// An entry, briefly: what a test compares.
fn show(entry: &Entry<V, Vec<u8>, &'static str>) -> String {
    match entry {
        Entry::Op(op) => {
            let kind = match op.kind {
                OpKind::Write { epoch } => format!("write@{}", epoch.id),
                OpKind::Read => "read".to_owned(),
            };
            let outcome = match &op.outcome {
                Outcome::Transferred(n) => format!("{n}"),
                Outcome::Failed(fault) => format!("failed by {}", fault.source),
                Outcome::NeverIssued { .. } => "never issued".to_owned(),
            };
            format!("op{} {kind} {outcome} {}", op.id.seq, op.context)
        }
        Entry::Durable { through } => format!("durable {}", through.id),
        Entry::Failed(failed) => format!("failed {}", failed.suspect.writes().len()),
        Entry::Blocked { through, .. } => format!("blocked {}", through.id),
        other => format!("{other:?}"),
    }
}

fn shown(rig: &mut Rig) -> Vec<String> {
    rig.entries().iter().map(show).collect()
}

#[test]
fn a_write_completes_with_its_count_buffer_and_context() {
    let mut rig = Rig::new();
    let id = rig.write(1, 0, b"hello").expect("accepted");
    rig.run();
    let entries = rig.entries();
    let [Entry::Op(op)] = entries.as_slice() else {
        panic!("one completion: {entries:?}");
    };
    assert_eq!(op.id, id);
    assert!(matches!(op.outcome, Outcome::Transferred(5)));
    assert_eq!(op.buffer.as_deref(), Some(b"hello".as_slice()));
    assert_eq!(op.context, "write");
    assert_eq!(rig.target().contents(), b"hello");
}

#[test]
fn a_read_returns_the_bytes_and_may_be_short() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"abc").expect("accepted");
    rig.run();
    rig.entries();
    rig.read(1, 8).expect("accepted");
    rig.run();
    let entries = rig.entries();
    let [Entry::Op(op)] = entries.as_slice() else {
        panic!("one completion: {entries:?}");
    };
    assert!(matches!(op.outcome, Outcome::Transferred(2)));
    assert_eq!(&op.buffer.as_deref().expect("owned")[..2], b"bc");
    assert_eq!(op.kind, OpKind::Read);
}

#[test]
fn accepted_operations_take_consecutive_ids_and_refused_ones_take_none() {
    let mut rig = Rig::with_queues(1, 16);
    let first = rig.write(1, 0, b"a").expect("accepted");
    let refused = rig.write(1, 1, b"b").expect_err("the queue is full");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::SubmitNeeded)
    ));
    rig.run();
    let second = rig.read(0, 1).expect("accepted");
    assert_eq!(second.seq, first.seq + 1);
}

#[test]
fn a_refused_push_hands_back_its_buffer_context_and_cause() {
    let mut rig = Rig::with_queues(8, 1);
    rig.write(1, 0, b"a").expect("accepted");
    let refused = rig
        .write(1, 0, b"kept")
        .expect_err("no room for a completion");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::DrainNeeded)
    ));
    assert_eq!(refused.buffer.as_deref(), Some(b"kept".as_slice()));
    assert_eq!(refused.context, "write");
}

#[test]
fn a_refused_push_records_nothing() {
    let mut rig = Rig::with_queues(1, 16);
    rig.write(1, 0, b"a").expect("accepted");
    rig.write(2, 1, b"b").expect_err("the queue is full");
    rig.run();
    assert_eq!(shown(&mut rig), ["op0 write@1 1 write"]);
    assert_eq!(rig.seal(2), DurabilityRequest::Submitted);
    rig.run();
    // Epoch 2's refused write was never recorded, so the seal waits for nothing of it.
    assert_eq!(shown(&mut rig), ["durable 2"]);
}

#[test]
fn an_unknown_target_is_refused_before_the_backend_sees_it() {
    let mut rig = Rig::new();
    let refused = rig
        .write_to(IoTargetKey(9), 1, 0, b"x")
        .expect_err("unknown");
    assert!(matches!(
        refused.reason,
        PushRefusal::UnknownFile(IoTargetKey(9))
    ));
    assert_eq!(refused.buffer.as_deref(), Some(b"x".as_slice()));
    assert_eq!(rig.backend.queued(), 0);
}

#[test]
fn a_write_at_or_below_the_seal_point_is_refused() {
    let mut rig = Rig::new();
    rig.seal(5);
    for epoch in [1, 5] {
        let refused = rig.write(epoch, 0, b"x").expect_err("sealed");
        assert!(
            matches!(
                refused.reason,
                PushRefusal::Sealed {
                    sealed_through: 5,
                    ..
                }
            ),
            "{:?}",
            refused.reason
        );
    }
    rig.write(6, 0, b"x").expect("above the seal");
}

#[test]
fn another_cores_lineage_is_refused() {
    let mut rig = Rig::new();
    let other = Core::new(Ticks::default(), Arc::new(NoDiagnostics));
    let foreign = Epoch::new(other.default_lineage(), 1);
    let backend = &mut rig.backend;
    let refused = rig
        .core
        .write(
            KEY,
            0,
            SubmissionData::Owned(vec![1]),
            foreign,
            "foreign",
            WriteCaching::Cached,
            |submission| backend.push(submission),
        )
        .expect_err("foreign");
    assert!(matches!(refused.reason, PushRefusal::UnknownLineage(_)));
    assert!(matches!(
        rig.core.make_durable_through(foreign),
        Err(SealError::UnknownLineage(_))
    ));
    assert!(rig.core.durable_through(other.default_lineage()).is_err());
    assert!(rig.core.sealed_through(other.default_lineage()).is_err());
    assert!(rig.core.epoch_state(foreign).is_err());
}

#[test]
fn a_seal_commits_after_its_writes_complete_and_then_reports_durable() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"one").expect("accepted");
    rig.write(2, 3, b"two").expect("accepted");
    assert_eq!(rig.seal(2), DurabilityRequest::Submitted);
    assert!(rig.target().durable_contents().is_empty());
    rig.run();
    assert_eq!(
        shown(&mut rig),
        ["op0 write@1 3 write", "op1 write@2 3 write", "durable 2"]
    );
    assert_eq!(rig.target().durable_contents(), b"onetwo");
    assert_eq!(
        rig.core.durable_through(rig.core.default_lineage()),
        Ok(Some(2))
    );
}

#[test]
fn a_seal_with_no_writes_is_durable_at_once() {
    let mut rig = Rig::new();
    let lineage = rig.core.default_lineage();
    let (answer, actions) = rig
        .core
        .make_durable_through(rig.epoch(3))
        .expect("the default lineage");
    assert_eq!(answer, DurabilityRequest::Submitted);
    assert!(actions.submit.is_empty(), "nothing to commit");
    assert!(actions.deliver);
    assert_eq!(shown(&mut rig), ["durable 3"]);
    assert_eq!(rig.core.sealed_through(lineage), Ok(Some(3)));
}

#[test]
fn a_seal_at_or_below_the_seal_point_reports_the_epochs_state() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(2);
    assert_eq!(
        rig.seal(1),
        DurabilityRequest::AlreadySealed(EpochState::Pending)
    );
    rig.run();
    assert_eq!(
        rig.seal(2),
        DurabilityRequest::AlreadySealed(EpochState::Durable)
    );
}

#[test]
fn epoch_states_follow_the_seal() {
    let mut rig = Rig::new();
    let state = |rig: &Rig, id| rig.core.epoch_state(rig.epoch(id)).expect("ours");
    rig.write(1, 0, b"x").expect("accepted");
    assert_eq!(state(&rig, 1), EpochState::Open);
    rig.seal(1);
    assert_eq!(state(&rig, 1), EpochState::Pending);
    assert_eq!(state(&rig, 2), EpochState::Open);
    rig.run();
    assert_eq!(state(&rig, 1), EpochState::Durable);
}

#[test]
fn a_seal_commits_each_target_its_writes_reached_once() {
    let mut rig = Rig::new();
    rig.attach(OTHER, MemoryTarget::new());
    rig.write(1, 0, b"a").expect("accepted");
    rig.write(1, 1, b"b").expect("accepted");
    rig.write_to(OTHER, 1, 0, b"c").expect("accepted");
    rig.backend.submit().expect("submit");
    let mut commits = Vec::new();
    while let Some(completion) = rig.backend.pop().expect("never fails") {
        let actions = rig.core.complete(completion);
        commits.extend(actions.submit.iter().map(|s| s.target));
        rig.act(actions);
    }
    assert!(commits.is_empty(), "nothing was sealed yet");
    let (_, actions) = rig
        .core
        .make_durable_through(rig.epoch(1))
        .expect("the default lineage");
    let mut targets: Vec<IoTargetKey> = actions.submit.iter().map(|s| s.target).collect();
    targets.sort_by_key(|key| key.0);
    assert_eq!(targets, [KEY, OTHER]);
    assert!(
        actions
            .submit
            .iter()
            .all(|s| matches!(s.op, SubmissionOp::Commit))
    );
}

#[test]
fn a_failed_commit_is_reported_as_a_failure_suspecting_its_writes() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"lost").expect("accepted");
    rig.seal(1);
    rig.backend.submit().expect("submit");
    let write = rig.backend.pop().expect("never fails").expect("the write");
    let actions = rig.core.complete(write);
    assert_eq!(actions.submit.len(), 1, "the commit is due");
    rig.act(actions);
    // The commit runs against a target the backend no longer has, and fails.
    rig.backend.detach(KEY).expect("attached");
    rig.run();
    let entries = rig.entries();
    assert!(matches!(entries[0], Entry::Op(_)));
    let Entry::Failed(failed) = &entries[1] else {
        panic!("a failure: {entries:?}");
    };
    let FailureCause::Commit { target, fault } = &failed.cause else {
        panic!("a commit's failure: {:?}", failed.cause);
    };
    assert_eq!(*target, KEY);
    assert_eq!(fault.source, crate::memory::MEMORY);
    assert_eq!(failed.suspect.writes().len(), 1);
    assert!(
        matches!(
            rig.core.epoch_state(rig.epoch(1)),
            Ok(EpochState::Blocked(_))
        ),
        "the failure blocks the epoch"
    );
}

#[test]
fn a_failed_write_completes_as_failed_and_is_never_committed() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.backend.detach(KEY).expect("attached");
    rig.run();
    assert_eq!(shown(&mut rig), ["op0 write@1 failed by memory write"]);
    rig.backend
        .attach(KEY, MemoryTarget::new())
        .expect("a new key");
    rig.seal(1);
    rig.run();
    assert_eq!(shown(&mut rig), ["durable 1"]);
    assert!(rig.target().durable_contents().is_empty());
}

#[test]
fn a_refused_commit_is_kept_and_emitted_again() {
    let mut rig = Rig::with_queues(1, 16);
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(1);
    rig.backend.submit().expect("submit");
    let write = rig.backend.pop().expect("never fails").expect("the write");
    // Fill the submission queue so the commit the write's completion makes due is refused.
    rig.read(0, 1).expect("accepted");
    let actions = rig.core.complete(write);
    assert_eq!(actions.submit.len(), 1);
    rig.act(actions);
    assert_eq!(rig.backend.queued(), 1, "only the read is queued");
    rig.backend.submit().expect("submit");
    let retried = rig.core.retry();
    assert_eq!(retried.submit.len(), 1, "the refused commit comes back");
    assert!(rig.core.retry().submit.is_empty(), "and only once");
    rig.act(retried);
    rig.run();
    let shown = shown(&mut rig);
    assert_eq!(shown.last().map(String::as_str), Some("durable 1"));
    assert_eq!(rig.target().durable_contents(), b"x");
}

#[test]
fn a_refused_commit_rides_on_the_next_actions() {
    let mut rig = Rig::with_queues(1, 16);
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(1);
    rig.backend.submit().expect("submit");
    let write = rig.backend.pop().expect("never fails").expect("the write");
    rig.read(0, 1).expect("accepted");
    let actions = rig.core.complete(write);
    rig.act(actions);
    rig.backend.submit().expect("submit");
    let read = rig.backend.pop().expect("never fails").expect("the read");
    let actions = rig.core.complete(read);
    assert_eq!(
        actions.submit.len(),
        1,
        "the refused commit, with the next actions"
    );
    rig.act(actions);
    rig.run();
    assert_eq!(rig.target().durable_contents(), b"x");
}

#[test]
fn deliver_marks_only_the_queue_becoming_non_empty() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"a").expect("accepted");
    rig.write(1, 1, b"b").expect("accepted");
    rig.backend.submit().expect("submit");
    let first = rig.backend.pop().expect("never fails").expect("a");
    let second = rig.backend.pop().expect("never fails").expect("b");
    let delivered = rig.core.complete(first);
    assert!(delivered.deliver, "the queue went from empty to one");
    let again = rig.core.complete(second);
    assert!(!again.deliver, "it was not empty");
    rig.entries();
    let (_, sealed) = rig
        .core
        .make_durable_through(rig.epoch(1))
        .expect("the default lineage");
    assert!(!sealed.deliver, "a seal waiting on its commit adds nothing");
    rig.act(sealed);
    rig.backend.submit().expect("submit");
    let commit = rig.backend.pop().expect("never fails").expect("the commit");
    assert!(
        rig.core.complete(commit).deliver,
        "durable, on an empty queue"
    );
}

#[test]
fn completions_precede_the_durable_they_lead_to() {
    let mut rig = Rig::new();
    for epoch in 1..=4 {
        rig.write(epoch, epoch, b"x").expect("accepted");
    }
    rig.seal(4);
    rig.run();
    let shown = shown(&mut rig);
    assert_eq!(shown.len(), 5);
    assert_eq!(shown[4], "durable 4");
}

#[test]
fn a_registered_region_completes_without_a_buffer() {
    let mut rig = Rig::new();
    rig.backend = MemoryBackend::new(nz(8), nz(16)).with_registered(vec![b"bytes".to_vec()]);
    rig.backend
        .attach(KEY, MemoryTarget::new())
        .expect("a new key");
    let epoch = rig.epoch(1);
    let backend = &mut rig.backend;
    let region = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 5,
    };
    rig.core
        .write(
            KEY,
            0,
            SubmissionData::Registered(region),
            epoch,
            "span",
            WriteCaching::Cached,
            |submission| backend.push(submission),
        )
        .expect("accepted");
    rig.run();
    let entries = rig.entries();
    let [Entry::Op(op)] = entries.as_slice() else {
        panic!("one completion: {entries:?}");
    };
    assert!(op.buffer.is_none());
    assert!(matches!(op.outcome, Outcome::Transferred(5)));
}

#[test]
fn a_refused_registered_region_hands_back_no_buffer() {
    let mut rig = Rig::new();
    let epoch = rig.epoch(1);
    let backend = &mut rig.backend;
    let region = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 1,
    };
    let refused = rig
        .core
        .write(
            KEY,
            0,
            SubmissionData::Registered(region),
            epoch,
            "span",
            WriteCaching::Cached,
            |submission| backend.push(submission),
        )
        .expect_err("no registration");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::Unsupported)
    ));
    assert!(refused.buffer.is_none());
}

#[test]
fn a_duplicate_target_hands_its_description_back() {
    let mut rig = Rig::new();
    let description = MemoryTarget::new()
        .with_domains([FlushDomain::new(b"d".as_slice())])
        .describe();
    let duplicate = rig
        .core
        .add_target(KEY, description.clone())
        .expect_err("taken");
    assert_eq!(duplicate.key, KEY);
    assert_eq!(duplicate.target, description);
    assert_eq!(rig.core.target(KEY), Some(&MemoryTarget::new().describe()));
}

#[test]
fn a_target_is_busy_while_an_operation_is_in_flight() {
    let mut rig = Rig::new();
    rig.read(0, 1).expect("accepted");
    let Err(RemoveTargetError::Busy(busy)) = rig.core.remove_target(KEY) else {
        panic!("busy");
    };
    assert_eq!(busy.in_flight, 1);
    assert_eq!(busy.uncovered, 0);
    rig.run();
    rig.core.remove_target(KEY).expect("nothing holds it");
}

#[test]
fn a_target_is_busy_until_its_writes_are_covered() {
    let mut rig = Rig::new();
    rig.write(3, 0, b"x").expect("accepted");
    rig.run();
    let Err(RemoveTargetError::Busy(busy)) = rig.core.remove_target(KEY) else {
        panic!("busy");
    };
    assert_eq!(busy.in_flight, 0);
    assert_eq!(busy.uncovered, 1);
    assert_eq!(busy.uncovered_through, [rig.epoch(3)]);
    rig.seal(3);
    rig.run();
    let description = rig.core.remove_target(KEY).expect("covered");
    assert_eq!(description, MemoryTarget::new().describe());
    assert!(rig.core.target(KEY).is_none());
    assert!(matches!(
        rig.core.remove_target(KEY),
        Err(RemoveTargetError::UnknownKey(KEY))
    ));
}

#[test]
fn a_target_is_busy_while_a_failure_suspects_its_writes() {
    let mut rig = Rig::new();
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(1);
    rig.backend.submit().expect("submit");
    let write = rig.backend.pop().expect("never fails").expect("the write");
    let actions = rig.core.complete(write);
    rig.act(actions);
    rig.backend.detach(KEY).expect("attached");
    rig.run();
    let Err(RemoveTargetError::Busy(busy)) = rig.core.remove_target(KEY) else {
        panic!("busy");
    };
    assert_eq!(busy.failures.len(), 1);
}

#[test]
#[should_panic(expected = "the backend completed a submission the core has no record of")]
fn a_completion_never_handed_out_is_panicked_on() {
    let mut rig = Rig::new();
    let _ = rig.core.complete(Completion {
        token: SubmissionToken(77),
        result: Ok(0),
        buffer: None,
    });
}

#[test]
#[should_panic(expected = "the backend completed a submission the core has no record of")]
fn a_completion_twice_is_panicked_on() {
    let mut rig = Rig::new();
    rig.read(0, 1).expect("accepted");
    rig.backend.submit().expect("submit");
    let completion = rig.backend.pop().expect("never fails").expect("the read");
    let token = completion.token;
    let _ = rig.core.complete(completion);
    let _ = rig.core.complete(Completion {
        token,
        result: Ok(0),
        buffer: None,
    });
}

#[test]
#[should_panic(expected = "a refusal handed back that is not one of the core's own submissions")]
fn a_consumer_operation_handed_back_as_refused_is_panicked_on() {
    let mut rig = Rig::new();
    let mut taken = None;
    let epoch = rig.epoch(1);
    rig.core
        .write(
            KEY,
            0,
            SubmissionData::Owned(vec![1]),
            epoch,
            "write",
            WriteCaching::Cached,
            |submission: Submission<Vec<u8>>| {
                taken = Some(submission.token);
                Ok(())
            },
        )
        .expect("accepted");
    rig.core.refused(crate::backend::Refused {
        cause: RefusalCause::SubmitNeeded,
        submission: Submission {
            token: taken.expect("pushed"),
            target: KEY,
            op: SubmissionOp::Commit,
        },
    });
}

#[test]
fn each_core_has_its_own_lineages() {
    let a = Core::new(Ticks::default(), Arc::new(NoDiagnostics));
    let b = Core::new(Ticks::default(), Arc::new(NoDiagnostics));
    assert_ne!(a.default_lineage(), b.default_lineage());
    assert_eq!(a.durable_through(a.default_lineage()), Ok(None));
    assert_eq!(a.sealed_through(a.default_lineage()), Ok(None));
}

#[test]
fn nothing_to_do_is_empty() {
    let mut rig = Rig::new();
    assert!(rig.core.retry().is_empty());
    let (_, actions) = rig
        .core
        .make_durable_through(rig.epoch(1))
        .expect("the default lineage");
    assert!(!actions.is_empty(), "it delivered");
}
