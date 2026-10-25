// Copyright (c) 2026 Mike Grier
//! The core driven over the in-memory backend: operations, completions, seals and commits,
//! refusals, the queue, and removing a target; gates, lineages, failures and the provider in the
//! modules below.

use std::num::NonZeroU32;
use std::sync::Arc;

use super::{CoreActions, DurableIoCore, Taken};
use crate::backend::{
    Completion, IoBackend, RefusalCause, RegisteredRegion, Submission, SubmissionData,
    SubmissionOp, SubmissionToken,
};
use crate::diagnostics::NoDiagnostics;
use crate::ids::{DioringIds, Lineage, OpId};
use crate::io_target::IoTarget;
use crate::memory::{MemoryBackend, MemoryTarget};
use crate::oracle::ConformanceOracle;
use crate::provider::ProviderRequest;
use crate::test_support::{Listed, Ticks};
use crate::types::{
    DurabilityRequest, Entry, Epoch, EpochState, FailureCause, FailureInfo, FlushDomain,
    HeldOperation, Hold, IoTargetKey, Leftover, OpKind, Outcome, PushError, PushRefusal,
    ReadOptions, RemoveTargetError, SealError, WriteOptions,
};

mod capacity;
mod coalescing;
mod failures;
mod flush_failures;
mod gates;
mod lineages;
mod lists;
mod pairs;
mod provider;
mod refusals;
mod scripted;
mod tables;
mod window;

const KEY: IoTargetKey = IoTargetKey(1);
const OTHER: IoTargetKey = IoTargetKey(2);

type Core = DurableIoCore<u64, Vec<u8>, &'static str, Ticks>;
type V = DioringIds<u64, u64>;
type Pushed = Result<OpId, PushError<V, Vec<u8>, &'static str>>;

/// What a call left to do, as lists of its own.
type Due = Taken<V, Vec<u8>>;

/// What a test acts on: what a call returned, its submissions and requests still in the core, or
/// those already taken out to look at.
trait Acts {
    fn taken(self, core: &mut Core) -> Due;
}

impl Acts for CoreActions {
    fn taken(self, core: &mut Core) -> Due {
        core.taken(self)
    }
}

impl Acts for Due {
    fn taken(self, _: &mut Core) -> Due {
        self
    }
}

fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("non-zero")
}

/// Each live lineage, as the visitor gives them.
fn listed(core: &Core) -> Vec<Listed<V>> {
    let mut listed = Vec::new();
    core.lineages(|info| listed.push(Listed::from(info)));
    listed
}

/// What holds the target under `key`, as the visitor names it.
fn target_holds(core: &Core, key: IoTargetKey) -> Vec<Hold<V>> {
    let mut holds = Vec::new();
    core.target_holds(key, |hold| holds.push(hold));
    holds
}

/// What holds `lineage` from being retired, as the visitor names it.
fn lineage_holds(core: &Core, lineage: Lineage) -> Vec<Hold<V>> {
    let mut holds = Vec::new();
    core.lineage_holds(lineage, |hold| holds.push(hold));
    holds
}

/// What `close` gave, by kind.
struct Closed {
    held: Vec<HeldOperation<V, Vec<u8>, &'static str>>,
    failures: Vec<FailureInfo<V>>,
    lineages: Vec<Listed<V>>,
}

/// Close `core`, checking it gives the held operations, then the failures, then the lineages.
fn closed(core: Core) -> Closed {
    let mut closed = Closed {
        held: Vec::new(),
        failures: Vec::new(),
        lineages: Vec::new(),
    };
    core.close(|leftover| match leftover {
        Leftover::Held(held) => {
            assert!(
                closed.failures.is_empty() && closed.lineages.is_empty(),
                "the held operations come first"
            );
            closed.held.push(held);
        }
        Leftover::Failure(failure) => {
            assert!(
                closed.lineages.is_empty(),
                "the failures come before the lineages"
            );
            closed.failures.push(failure);
        }
        Leftover::Lineage(lineage) => closed.lineages.push(Listed::from(lineage)),
    });
    closed
}

/// A core and a backend, with the driver's part played here.
struct Rig {
    core: Core,
    backend: MemoryBackend,
    /// Every provider request the core has made, in order.
    requests: Vec<ProviderRequest<V>>,
    /// The contract's rules, over every push, seal, heal and entry the rig sees (DI-D-24).
    oracle: ConformanceOracle<V, &'static str>,
    /// Each failure's suspect set, as the core lent it when its `Failed` was popped (DI-D-108).
    suspected: std::collections::HashMap<u64, Vec<crate::types::SuspectWrite<V>>>,
}

impl Rig {
    fn new() -> Self {
        Self::with_queues(8, 16)
    }

    fn with_queues(submissions: u32, completions: u32) -> Self {
        let mut rig = Self::over(
            Core::new(
                Ticks::default(),
                Arc::new(NoDiagnostics),
                crate::test_support::CAPACITY,
            )
            .expect("a valid capacity"),
        );
        rig.backend = MemoryBackend::new(nz(submissions), nz(completions));
        rig.attach(KEY, MemoryTarget::new());
        rig
    }

    /// A rig over a backend that does what `script` says, with the key attached.
    fn scripted(script: crate::memory::FaultScript) -> Self {
        let mut rig = Self::over(
            Core::new(
                Ticks::default(),
                Arc::new(NoDiagnostics),
                crate::test_support::CAPACITY,
            )
            .expect("a valid capacity"),
        );
        rig.backend = MemoryBackend::new(nz(8), nz(16)).with_script(script);
        rig.attach(KEY, MemoryTarget::new());
        rig
    }

    /// `core`, with no target, over a backend with room to spare.
    fn over(core: Core) -> Self {
        Self {
            core,
            backend: MemoryBackend::new(nz(8), nz(16)),
            requests: Vec::new(),
            oracle: ConformanceOracle::new(),
            suspected: std::collections::HashMap::new(),
        }
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
        self.write_with(key, epoch, offset, bytes, WriteOptions::new())
    }

    /// A write of `bytes`, tagged `epoch` in any lineage, with `options`.
    fn write_with(
        &mut self,
        key: IoTargetKey,
        epoch: Epoch<V>,
        offset: u64,
        bytes: &[u8],
        options: WriteOptions<V>,
    ) -> Pushed {
        let pushed = self.core.write(
            key,
            offset,
            SubmissionData::Owned(bytes.to_vec()),
            epoch,
            "write",
            options,
            &mut self.backend,
        );
        self.report(
            &pushed,
            OpKind::Write { epoch },
            bytes.len(),
            "write",
            options.gate,
        );
        pushed
    }

    /// Tell the oracle of an accepted push.
    fn report(
        &mut self,
        pushed: &Pushed,
        kind: OpKind<V>,
        length: usize,
        context: &'static str,
        gate: Option<Epoch<V>>,
    ) {
        let Ok(id) = pushed else {
            return;
        };
        let length = u32::try_from(length).expect("a test buffer's length fits a u32");
        let accepted = match gate {
            None => self.oracle.pushed(*id, kind, length, context),
            Some(gate) => self.oracle.pushed_gated(*id, kind, length, context, gate),
        };
        accepted.expect("the oracle accepts the push");
    }

    /// A read of `length` bytes from `KEY`, with `options`.
    fn read_with(&mut self, offset: u64, length: usize, options: ReadOptions<V>) -> Pushed {
        let pushed = self.core.read(
            KEY,
            offset,
            SubmissionData::Owned(vec![0; length]),
            "read",
            options,
            &mut self.backend,
        );
        self.report(&pushed, OpKind::Read, length, "read", options.gate);
        pushed
    }

    /// Seal `lineage` through `through`, acting on what that made due.
    fn seal_in(&mut self, lineage: Lineage, through: u64) -> DurabilityRequest<V> {
        let (answer, actions) = self.sealed(Epoch::new(lineage, through));
        self.act(actions);
        answer
    }

    /// Seal through `through`, telling the oracle of a new seal: the answer, and what it made due,
    /// not yet acted on.
    fn sealed(&mut self, through: Epoch<V>) -> (DurabilityRequest<V>, Due) {
        let (answer, actions) = self
            .core
            .make_durable_through(through)
            .expect("a live lineage of this core");
        if answer == DurabilityRequest::Submitted {
            self.oracle.sealed(through);
        }
        (answer, self.core.taken(actions))
    }

    /// Hand `completion` to the core: what that made due, taken out to look at.
    fn completed(&mut self, completion: Completion<Vec<u8>>) -> Due {
        let actions = self.core.complete(completion);
        self.core.taken(actions)
    }

    /// The submissions refused before, taken out to look at.
    fn retried(&mut self) -> Due {
        let actions = self.core.retry();
        self.core.taken(actions)
    }

    fn write(&mut self, epoch: u64, offset: u64, bytes: &[u8]) -> Pushed {
        self.write_to(KEY, epoch, offset, bytes)
    }

    fn read(&mut self, offset: u64, length: usize) -> Pushed {
        self.read_with(offset, length, ReadOptions::new())
    }

    fn seal(&mut self, through: u64) -> DurabilityRequest<V> {
        let lineage = self.core.default_lineage();
        self.seal_in(lineage, through)
    }

    /// Push what the core asks for, handing refusals back and acting on what they make due, and
    /// keep its provider requests: the engine's `act`, played here.
    fn act(&mut self, actions: impl Acts) -> bool {
        let mut deliver = false;
        let mut pending = std::collections::VecDeque::from([actions.taken(&mut self.core)]);
        while let Some(actions) = pending.pop_front() {
            deliver |= actions.deliver;
            self.requests.extend(actions.requests);
            for submission in actions.submit {
                if let Err(refused) = self.backend.push(submission) {
                    let more = self.core.refused(refused);
                    pending.push_back(self.core.taken(more));
                }
            }
        }
        deliver
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

    /// Every entry, popped one at a time and each reported to the oracle: a `Failed`'s suspect set
    /// is read from the core as it is popped, kept in `suspected`, and given to the oracle.
    fn entries(&mut self) -> Vec<Entry<V, Vec<u8>, &'static str>> {
        std::iter::from_fn(|| self.pop_entry()).collect()
    }

    /// The next entry, reported to the oracle, a `Failed`'s suspect set read as `entries` reads it.
    fn pop_entry(&mut self) -> Option<Entry<V, Vec<u8>, &'static str>> {
        let entry = self.core.pop()?;
        self.oracle
            .observe(&entry)
            .expect("the oracle accepts the entry");
        if let Entry::Failed(failed) = &entry {
            let suspect = suspects_of(&self.core, failed.id);
            self.oracle
                .suspects(failed.id, &suspect)
                .expect("the oracle accepts the suspect set");
            self.suspected.insert(failed.id.seq, suspect);
        }
        Some(entry)
    }

    fn target(&self) -> &MemoryTarget {
        self.backend.target(KEY).expect("attached")
    }
}

/// The writes `failure` suspects, as `core` lends them.
fn suspects_of(core: &Core, failure: crate::ids::FailureId) -> Vec<crate::types::SuspectWrite<V>> {
    let mut writes = Vec::new();
    core.failure_suspects(failure, |write| writes.push(write))
        .expect("the core holds the failure's records");
    writes
}

/// The markings `failure` has gained, as `core` lends them.
fn markings_of(core: &Core, failure: crate::ids::FailureId) -> Vec<crate::types::Marking<V>> {
    let mut markings = Vec::new();
    core.failure_markings(failure, |marking| markings.push(marking))
        .expect("the core holds the failure's records");
    markings
}

/// An entry, briefly: what a test compares, a failure by the size of its suspect set as
/// `suspected` holds it.
fn show(
    entry: &Entry<V, Vec<u8>, &'static str>,
    suspected: &std::collections::HashMap<u64, Vec<crate::types::SuspectWrite<V>>>,
) -> String {
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
        Entry::Failed(failed) => format!("failed {}", suspected[&failed.id.seq].len()),
        Entry::Blocked { through, .. } => format!("blocked {}", through.id),
        other => format!("{other:?}"),
    }
}

fn shown(rig: &mut Rig) -> Vec<String> {
    let entries = rig.entries();
    entries
        .iter()
        .map(|entry| show(entry, &rig.suspected))
        .collect()
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
    let other = Core::new(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        crate::test_support::CAPACITY,
    )
    .expect("a valid capacity");
    let foreign = Epoch::new(other.default_lineage(), 1);
    let refused = rig
        .core
        .write(
            KEY,
            0,
            SubmissionData::Owned(vec![1]),
            foreign,
            "foreign",
            WriteOptions::new(),
            &mut rig.backend,
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
    let (answer, actions) = rig.sealed(rig.epoch(3));
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
        let actions = rig.completed(completion);
        commits.extend(actions.submit.iter().map(|s| s.target));
        rig.act(actions);
    }
    assert!(commits.is_empty(), "nothing was sealed yet");
    let (_, actions) = rig.sealed(rig.epoch(1));
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
    let actions = rig.completed(write);
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
    assert_eq!(rig.suspected[&failed.id.seq].len(), 1);
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
    let actions = rig.completed(write);
    assert_eq!(actions.submit.len(), 1);
    rig.act(actions);
    assert_eq!(rig.backend.queued(), 1, "only the read is queued");
    rig.backend.submit().expect("submit");
    let retried = rig.retried();
    assert_eq!(retried.submit.len(), 1, "the refused commit comes back");
    assert!(rig.retried().submit.is_empty(), "and only once");
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
    let actions = rig.completed(read);
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
    let (_, sealed) = rig.sealed(rig.epoch(1));
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
    let region = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 5,
    };
    let pushed = rig.core.write(
        KEY,
        0,
        SubmissionData::Registered(region),
        epoch,
        "span",
        WriteOptions::new(),
        &mut rig.backend,
    );
    rig.report(&pushed, OpKind::Write { epoch }, 5, "span", None);
    pushed.expect("accepted");
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
            WriteOptions::new(),
            &mut rig.backend,
        )
        .expect_err("no registration");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::Unsupported)
    ));
    assert!(refused.buffer.is_none());
}

#[test]
fn domains_are_interned_once_per_distinct_bytes() {
    // Moved from the driver's tests with the interning itself (`DI-3.10.3.5.6.2.6`).
    let core = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        crate::test_support::CAPACITY,
        vec![domain("C"), domain("D")],
    )
    .expect("no repeats");
    let mut rig = Rig::over(core);
    rig.attach(
        KEY,
        MemoryTarget::new().with_domains([domain("A"), domain("B")]),
    );
    rig.attach(
        OTHER,
        MemoryTarget::new().with_domains([domain("B"), domain("C"), domain("B")]),
    );
    rig.attach(IoTargetKey(3), MemoryTarget::new());
    let core = &rig.core;
    assert_eq!(core.domains.len(), 4, "A, B, C and D, once each");
    let id = |name: &str| core.domains[&domain(name)];
    let of = |key: IoTargetKey| {
        core.durability
            .lists()
            .get(core.targets[&key].domains)
            .to_vec()
    };

    let mut one = vec![id("A"), id("B")];
    one.sort_unstable();
    assert_eq!(&*of(KEY), one.as_slice());
    let mut two = vec![id("B"), id("C")];
    two.sort_unstable();
    assert_eq!(&*of(OTHER), two.as_slice(), "a repeated domain counts once");
    assert!(
        of(IoTargetKey(3)).is_empty(),
        "a target declared with none stays unknown"
    );

    let mut provider = vec![id("C"), id("D")];
    provider.sort_unstable();
    assert_eq!(
        &*core.served,
        provider.as_slice(),
        "a provider's domain shares its id with a target's"
    );
}

/// A flush domain named `name`.
fn domain(name: &str) -> FlushDomain {
    FlushDomain::new(name.as_bytes().to_vec())
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
    assert_eq!(
        target_holds(&rig.core, KEY),
        [Hold::Uncovered(rig.epoch(3))]
    );
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
    assert_eq!(busy.failures, 1);
    assert!(matches!(
        target_holds(&rig.core, KEY)[..],
        [Hold::Failure(_)]
    ));
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
    rig.write(1, 0, b"x").expect("accepted");
    rig.backend.submit().expect("submit");
    // The write's token, read off its completion, which the core is never shown.
    let token = rig
        .backend
        .pop()
        .expect("never fails")
        .expect("the write")
        .token;
    let _ = rig.core.refused(crate::backend::Refused {
        cause: RefusalCause::SubmitNeeded,
        submission: Submission {
            token,
            target: KEY,
            op: SubmissionOp::Commit,
        },
    });
}

#[test]
fn each_core_has_its_own_lineages() {
    let a = Core::new(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        crate::test_support::CAPACITY,
    )
    .expect("a valid capacity");
    let b = Core::new(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        crate::test_support::CAPACITY,
    )
    .expect("a valid capacity");
    assert_ne!(a.default_lineage(), b.default_lineage());
    assert_eq!(a.durable_through(a.default_lineage()), Ok(None));
    assert_eq!(a.sealed_through(a.default_lineage()), Ok(None));
}

#[test]
fn nothing_to_do_is_empty() {
    let mut rig = Rig::new();
    assert!(rig.retried().is_empty());
    let (_, actions) = rig.sealed(rig.epoch(1));
    assert!(!actions.is_empty(), "it delivered");
}
