// Copyright (c) 2026 Mike Grier
//! The engine's doorbell (DI-7+.2): a backend only queues what it is pushed, the engine rings it
//! once for a batch, and a doorbell that failed, or completions that could not be read, are
//! something the consumer is told once.

use std::io;
use std::sync::Arc;

use super::{Core, ONE, TWO, at, entries, nz, plain, write};
use crate::backend::{
    Completion, DuplicateTarget, IoBackend, RefusalCause, Refused, Submission, SubmissionOp,
};
use crate::diagnostics::NoDiagnostics;
use crate::engine::Engine;
use crate::io_target::IoTarget;
use crate::memory::{MemoryBackend, MemoryNotification, MemoryTarget};
use crate::oracle::ConformanceOracle;
use crate::test_support::Ticks;
use crate::types::{DurabilityRequest, Entry, FlushDomain, IoFault, IoTargetKey};

/// The memory backend, with its doorbell and its reading of completions failing on demand, and
/// the doorbell rings counted.
struct Scripted {
    inner: MemoryBackend,
    fail_submit: bool,
    /// Fail the next doorbell that has something queued to issue, once; those after it work.
    fail_next_queued_submit: bool,
    fail_pop: bool,
    rings: u32,
    pops_failed: u32,
}

impl IoBackend for Scripted {
    type Target = MemoryTarget;
    type Buffer = Vec<u8>;
    type Notification = MemoryNotification;

    fn attach(
        &mut self,
        key: IoTargetKey,
        target: MemoryTarget,
    ) -> Result<(), DuplicateTarget<MemoryTarget>> {
        self.inner.attach(key, target)
    }

    fn detach(&mut self, key: IoTargetKey) -> Option<MemoryTarget> {
        self.inner.detach(key)
    }

    fn registered_buffers(&self) -> u32 {
        self.inner.registered_buffers()
    }

    fn check(&self, target: IoTargetKey, op: &SubmissionOp<Vec<u8>>) -> Result<(), RefusalCause> {
        self.inner.check(target, op)
    }

    fn push(&mut self, submission: Submission<Vec<u8>>) -> Result<(), Refused<Vec<u8>>> {
        self.inner.push(submission)
    }

    fn submit(&mut self) -> Result<(), RefusalCause> {
        self.rings += 1;
        if self.fail_submit {
            return Err(RefusalCause::Busy);
        }
        if self.fail_next_queued_submit && self.inner.queued() > 0 {
            self.fail_next_queued_submit = false;
            return Err(RefusalCause::Busy);
        }
        self.inner.submit()
    }

    fn pop(&mut self) -> Result<Option<Completion<Vec<u8>>>, IoFault> {
        if self.fail_pop {
            self.pops_failed += 1;
            return Err(IoFault {
                source: "test",
                code: None,
                error: crate::report_io_error(io::Error::other(format!(
                    "pop {}",
                    self.pops_failed
                ))),
            });
        }
        self.inner.pop()
    }

    fn notification(&self) -> Result<MemoryNotification, IoFault> {
        self.inner.notification()
    }
}

type Eng = Engine<u64, Scripted, u32, Ticks>;

/// An engine over two memory targets behind the scripted backend.
fn scripted(submissions: u32, completions: u32) -> Eng {
    let backend = Scripted {
        inner: MemoryBackend::new(nz(submissions), nz(completions)),
        fail_submit: false,
        fail_next_queued_submit: false,
        fail_pop: false,
        rings: 0,
        pops_failed: 0,
    };
    let core = Core::new(Ticks::default(), Arc::new(NoDiagnostics));
    let mut engine = Engine::new(core, backend);
    for key in [ONE, TWO] {
        let target = MemoryTarget::new().with_domains([FlushDomain::new(b"log".as_slice())]);
        engine
            .core
            .add_target(key, target.describe())
            .expect("a new key");
        engine.backend.attach(key, target).expect("a new key");
    }
    engine
}

/// Write `bytes` to `key` at epoch 1, as a consumer's push does.
fn push(engine: &mut Eng, key: IoTargetKey, bytes: &[u8]) {
    let epoch = crate::types::Epoch::new(engine.core.default_lineage(), 1);
    engine
        .write(
            key,
            0,
            crate::backend::SubmissionData::Owned(bytes.to_vec()),
            epoch,
            0,
            crate::types::WriteOptions::new(),
        )
        .expect("accepted");
}

#[test]
fn a_consumers_write_is_issued_by_the_engines_doorbell() {
    let mut engine = plain(8, 16);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, ONE, b"one");
    assert_eq!(engine.backend.queued(), 0, "pushed, and issued");
    assert_eq!(engine.backend.completed(), 1, "and it ran");
    assert!(!engine.take_attention(), "the doorbell did not fail");
}

#[test]
fn a_batch_of_pushes_is_one_doorbell() {
    let mut engine = scripted(8, 16);
    push(&mut engine, ONE, b"one");
    push(&mut engine, TWO, b"two");
    engine.drain();
    engine.core.pop();
    engine.core.pop();
    let rings = engine.backend.rings;
    engine
        .make_durable_through(crate::types::Epoch::new(engine.core.default_lineage(), 1))
        .expect("the default lineage");
    assert_eq!(
        engine.backend.rings - rings,
        1,
        "a commit for each target, pushed in one act, rung once"
    );
    assert_eq!(engine.backend.inner.completed(), 2, "and both ran");
}

#[test]
fn a_full_queue_is_rung_and_the_push_made_once_more() {
    // A one-entry submission queue: the second commit is refused for want of room, which the
    // doorbell makes, so the act pushes it again and neither is left kept.
    let mut engine = plain(1, 16);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, ONE, b"one");
    write(&mut engine, &mut oracle, TWO, b"two");
    engine.drain();
    entries(&mut engine, &mut oracle);
    engine
        .make_durable_through(at(&engine, 1))
        .expect("the default lineage");
    assert_eq!(engine.backend.queued(), 0);
    assert_eq!(engine.backend.completed(), 2, "both commits ran");
    assert!(engine.core.retry().is_empty(), "nothing was kept");
}

#[test]
fn a_failed_doorbell_for_a_push_is_noted_once_and_the_push_stands() {
    let mut engine = scripted(8, 16);
    engine.backend.fail_submit = true;
    push(&mut engine, ONE, b"one");
    assert_eq!(engine.backend.inner.queued(), 1, "the push stands, queued");
    assert!(
        engine.take_attention(),
        "the driver is to tell the consumer"
    );
    assert!(!engine.take_attention(), "once");

    engine.backend.fail_submit = false;
    assert!(engine.drain(), "the consumer's retry issues it");
    assert_eq!(engine.backend.inner.queued(), 0);
    assert!(engine.core.pop().is_some(), "and it completed");
}

#[test]
fn a_failed_doorbell_for_an_act_is_reported_as_something_to_deliver() {
    let mut engine = scripted(8, 16);
    push(&mut engine, ONE, b"one");
    engine.drain();
    engine.core.pop();
    engine.backend.fail_submit = true;
    let (_, deliver) = engine
        .make_durable_through(crate::types::Epoch::new(engine.core.default_lineage(), 1))
        .expect("the default lineage");
    assert!(deliver, "the commit is queued and the doorbell failed");
    assert_eq!(engine.backend.inner.queued(), 1);

    engine.backend.fail_submit = false;
    assert!(engine.drain());
    assert_eq!(engine.backend.inner.queued(), 0, "issued by the next one");
}

#[test]
fn a_drain_goes_on_to_issue_what_a_doorbell_it_rang_itself_failed_to() {
    // The write's completion makes the target's commit due, and the act that pushes the commit
    // rings the doorbell, which fails once. The commit stays queued, and only another pass of the
    // same drain issues it and pops what it completes: a drain that stopped after one pass would
    // return with the commit still queued.
    let mut engine = scripted(8, 16);
    push(&mut engine, ONE, b"one");
    let (answer, deliver) = engine
        .make_durable_through(crate::types::Epoch::new(engine.core.default_lineage(), 1))
        .expect("the default lineage");
    assert_eq!(answer, DurabilityRequest::Submitted);
    assert!(!deliver, "the write is still in flight");
    engine.backend.fail_next_queued_submit = true;

    assert!(engine.drain(), "entries became poppable");
    assert!(
        !engine.backend.fail_next_queued_submit,
        "the commit's doorbell was the one that failed"
    );
    assert_eq!(
        engine.backend.inner.queued(),
        0,
        "a later pass of the same drain issued it"
    );
    let entries: Vec<_> = std::iter::from_fn(|| engine.core.pop()).collect();
    assert!(
        matches!(entries.last(), Some(Entry::Durable { .. })),
        "{entries:?}"
    );
}

#[test]
fn a_drain_whose_doorbell_fails_says_so_and_keeps_what_is_queued() {
    let mut engine = scripted(8, 16);
    engine.backend.fail_submit = true;
    push(&mut engine, ONE, b"one");
    engine.take_attention();
    assert!(engine.drain(), "told, with nothing popped");
    assert_eq!(engine.backend.inner.queued(), 1);
    assert!(engine.core.pop().is_none());
}

#[test]
fn a_backend_that_cannot_read_its_completions_ends_the_drain_and_its_first_fault_is_kept() {
    let mut engine = scripted(8, 16);
    push(&mut engine, ONE, b"one");
    engine.backend.fail_pop = true;
    assert!(engine.drain(), "told, so the consumer's pop can report it");
    assert!(engine.drain(), "again");
    let fault = engine.take_fault().expect("kept");
    assert_eq!(fault.source, "test");
    assert_eq!(fault.error.to_string(), "pop 1", "the first, not the last");
    assert!(engine.take_fault().is_none(), "reported once");

    engine.backend.fail_pop = false;
    assert!(engine.drain(), "what was not read is there for the next");
    assert!(engine.core.pop().is_some());
}

#[test]
fn a_fault_the_next_read_got_past_is_not_reported() {
    let mut engine = scripted(8, 16);
    push(&mut engine, ONE, b"one");
    engine.backend.fail_pop = true;
    assert!(engine.drain());
    engine.backend.fail_pop = false;
    assert!(engine.drain(), "the completion was there all along");
    assert!(engine.take_fault().is_none(), "a read has worked since");
}
