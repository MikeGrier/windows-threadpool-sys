// Copyright (c) 2026 Mike Grier
//! The engine over the in-memory backend: acting, refusals kept and re-emitted or ended, provider
//! requests kept, and draining to empty.

use std::num::NonZeroU32;
use std::sync::Arc;

use super::Engine;
use crate::backend::{IoBackend, SubmissionData};
use crate::diagnostics::NoDiagnostics;
use crate::ids::DioringIds;
use crate::io_core::DurableIoCore;
use crate::io_target::IoTarget;
use crate::memory::{MemoryBackend, MemoryTarget};
use crate::oracle::ConformanceOracle;
use crate::provider::DomainOutcome;
use crate::test_support::Ticks;
use crate::types::{
    DurabilityRequest, Entry, Epoch, FlushDomain, IoTargetKey, OpKind, ReadOptions, WriteOptions,
};

const ONE: IoTargetKey = IoTargetKey(1);
const TWO: IoTargetKey = IoTargetKey(2);

type V = DioringIds<u64, u64>;
type Core = DurableIoCore<u64, Vec<u8>, u32, Ticks>;
type Eng = Engine<u64, MemoryBackend, u32, Ticks>;

fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("non-zero")
}

/// An engine over two memory targets, with the given queue sizes.
fn engine(submissions: u32, completions: u32, core: Core) -> Eng {
    let mut engine = Engine::new(core, MemoryBackend::new(nz(submissions), nz(completions)));
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

fn plain(submissions: u32, completions: u32) -> Eng {
    engine(
        submissions,
        completions,
        Core::new(Ticks::default(), Arc::new(NoDiagnostics)),
    )
}

fn at(engine: &Eng, id: u64) -> Epoch<V> {
    Epoch::new(engine.core.default_lineage(), id)
}

/// Write `bytes` to `key` at epoch 1, telling the oracle.
fn write(engine: &mut Eng, oracle: &mut ConformanceOracle<V, u32>, key: IoTargetKey, bytes: &[u8]) {
    let epoch = at(engine, 1);
    let context = u32::try_from(key.0).expect("small");
    let id = engine
        .write(
            key,
            0,
            SubmissionData::Owned(bytes.to_vec()),
            epoch,
            context,
            WriteOptions::new(),
        )
        .expect("accepted");
    let length = u32::try_from(bytes.len()).expect("small");
    oracle
        .pushed(id, OpKind::Write { epoch }, length, context)
        .expect("the oracle accepts the push");
}

fn entries(
    engine: &mut Eng,
    oracle: &mut ConformanceOracle<V, u32>,
) -> Vec<Entry<V, Vec<u8>, u32>> {
    let entries: Vec<_> = std::iter::from_fn(|| engine.core.pop()).collect();
    for entry in &entries {
        oracle.observe(entry).expect("the oracle accepts the entry");
    }
    entries
}

#[test]
fn a_drain_completes_writes_commits_and_reports_durable() {
    let mut engine = plain(8, 16);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, ONE, b"one");
    write(&mut engine, &mut oracle, TWO, b"two");
    let (answer, deliver) = engine
        .make_durable_through(at(&engine, 1))
        .expect("the default lineage");
    assert_eq!(answer, DurabilityRequest::Submitted);
    assert!(!deliver, "the writes are still in flight");
    oracle.sealed(at(&engine, 1));
    assert!(engine.drain(), "entries became poppable");
    let entries = entries(&mut engine, &mut oracle);
    assert!(
        matches!(entries.last(), Some(Entry::Durable { .. })),
        "{entries:?}"
    );
    for key in [ONE, TWO] {
        assert!(
            !engine
                .backend
                .target(key)
                .expect("attached")
                .durable_contents()
                .is_empty()
        );
    }
    assert_eq!(engine.backend.queued(), 0);
    assert_eq!(engine.backend.completed(), 0, "drained to empty");
    oracle.finish().expect("every operation completed");
}

#[test]
fn a_drain_with_nothing_to_pop_delivers_nothing() {
    let mut engine = plain(8, 16);
    assert!(!engine.drain());
    assert!(engine.core.pop().is_none());
}

#[test]
fn a_commit_the_backend_refuses_is_kept_and_issued_by_a_later_drain() {
    // A one-entry submission queue: the second target's commit cannot be queued beside the first.
    let mut engine = plain(1, 16);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, ONE, b"one");
    engine.drain();
    write(&mut engine, &mut oracle, TWO, b"two");
    engine.drain();
    entries(&mut engine, &mut oracle);
    let (_, _) = engine
        .make_durable_through(at(&engine, 1))
        .expect("the default lineage");
    oracle.sealed(at(&engine, 1));
    assert_eq!(engine.backend.queued(), 1, "only one commit fit");
    assert!(engine.drain());
    let entries = entries(&mut engine, &mut oracle);
    assert!(
        matches!(entries.last(), Some(Entry::Durable { .. })),
        "{entries:?}"
    );
    for key in [ONE, TWO] {
        assert!(
            !engine
                .backend
                .target(key)
                .expect("attached")
                .durable_contents()
                .is_empty(),
            "{key:?} committed"
        );
    }
    oracle.finish().expect("every operation completed");
}

#[test]
fn a_commit_the_backend_refuses_for_good_is_ended_and_what_that_makes_due_is_acted_on() {
    // DI-D-54: the refusal's own actions are acted on in turn, so the failure it makes due is
    // delivered by the call that met the refusal.
    let mut engine = plain(8, 16);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, ONE, b"one");
    engine.drain();
    entries(&mut engine, &mut oracle);
    engine.backend.detach(ONE).expect("attached");
    let (_, deliver) = engine
        .make_durable_through(at(&engine, 1))
        .expect("the default lineage");
    oracle.sealed(at(&engine, 1));
    assert!(deliver, "the ended commit's failure became poppable");
    assert_eq!(engine.backend.queued(), 0, "nothing was pushed");
    assert!(engine.core.retry().is_empty(), "nothing was kept");
    let entries = entries(&mut engine, &mut oracle);
    assert!(
        matches!(entries.as_slice(), [Entry::Failed(_)]),
        "{entries:?}"
    );
    oracle.finish().expect("every operation completed");
}

#[test]
fn a_drain_keeps_provider_requests_for_the_driver() {
    let core = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        vec![FlushDomain::new(b"log".as_slice())],
    )
    .expect("no repeats");
    let mut engine = engine(8, 16, core);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, ONE, b"one");
    engine
        .make_durable_through(at(&engine, 1))
        .expect("the default lineage");
    oracle.sealed(at(&engine, 1));
    engine.drain();
    let requests = engine.take_requests();
    assert_eq!(requests.len(), 1, "the seal asked the provider once");
    assert!(engine.take_requests().is_empty(), "taken once");
    entries(&mut engine, &mut oracle);
    let request = &requests[0];
    let actions = engine.core.provided(
        request.lineage,
        request.seal,
        &FlushDomain::new(b"log".as_slice()),
        DomainOutcome::Succeeded,
    );
    assert!(engine.act(actions));
    let entries = entries(&mut engine, &mut oracle);
    assert!(
        matches!(entries.as_slice(), [Entry::Durable { .. }]),
        "{entries:?}"
    );
    oracle.finish().expect("every operation completed");
}

#[test]
fn a_read_goes_through_the_engine() {
    let mut engine = plain(8, 16);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, ONE, b"abc");
    engine.drain();
    let id = engine
        .read(
            ONE,
            0,
            SubmissionData::Owned(vec![0; 3]),
            9,
            ReadOptions::new(),
        )
        .expect("accepted");
    oracle
        .pushed(id, OpKind::Read, 3, 9)
        .expect("the oracle accepts the push");
    assert!(
        !engine.drain(),
        "the write's completion is still unpopped, so not a transition"
    );
    let entries = entries(&mut engine, &mut oracle);
    let Some(Entry::Op(read)) = entries.last() else {
        panic!("the read last: {entries:?}");
    };
    assert_eq!(read.buffer.as_deref(), Some(b"abc".as_slice()));
    oracle.finish().expect("every operation completed");
}
