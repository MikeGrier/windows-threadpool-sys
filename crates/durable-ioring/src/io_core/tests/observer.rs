// Copyright (c) 2026 Mike Grier
//! The failure observer (DI-D-113): told of each failure, then its suspect writes in push order,
//! before its `Failed` can be popped; then of each marking once for each failure it goes to, in
//! observation order, a resolved failure told nothing more. Every rig in these tests also compares
//! what its observer was told of a failure's suspects with what the core holds, as each `Failed`
//! is popped, and the oracle checks the markings it was told of when the failure is resolved.

use std::sync::{Arc, Mutex};

use super::{Core, KEY, Rig, Ticks, V, nz};
use crate::backend::IoBackend;
use crate::diagnostics::NoDiagnostics;
use crate::ids::FailureToken;
use crate::io_target::IoTarget;
use crate::memory::{MemoryBackend, MemoryTarget};
use crate::observer::FailureObserver;
use crate::oracle::{ConformanceOracle, ObservedFailures};
use crate::types::{
    Entry, FailureCause, ImportScope, Marking, MarkingKind, Resolution, SuspectWrite,
};

/// Every call the observer gets, briefly, in the order made.
#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Log {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("not poisoned"))
    }
}

impl FailureObserver<V> for Log {
    fn failed(&self, failure: crate::ids::FailureId, cause: &FailureCause<V>, _observed: u64) {
        let kind = match cause {
            FailureCause::Imported { .. } => "imported",
            _ => "other",
        };
        let line = format!("failed f{} {kind}", failure.seq);
        self.0.lock().expect("not poisoned").push(line);
    }

    fn suspected(&self, failure: crate::ids::FailureId, write: SuspectWrite<V>) {
        let line = format!("suspect f{} op{}", failure.seq, write.op.seq);
        self.0.lock().expect("not poisoned").push(line);
    }

    fn marked(&self, failure: crate::ids::FailureId, marking: &Marking<V>) {
        let kind = match marking.kind {
            MarkingKind::Covered { .. } => "covered",
            _ => "other",
        };
        let line = format!("marked f{} op{} {kind}", failure.seq, marking.write.seq);
        self.0.lock().expect("not poisoned").push(line);
    }
}

/// Tells two observers: the rig's recorder, and a log.
struct Both(Arc<ObservedFailures<V>>, Arc<Log>);

impl FailureObserver<V> for Both {
    fn failed(&self, failure: crate::ids::FailureId, cause: &FailureCause<V>, observed: u64) {
        self.0.failed(failure, cause, observed);
        self.1.failed(failure, cause, observed);
    }

    fn suspected(&self, failure: crate::ids::FailureId, write: SuspectWrite<V>) {
        self.0.suspected(failure, write);
        self.1.suspected(failure, write);
    }

    fn marked(&self, failure: crate::ids::FailureId, marking: &Marking<V>) {
        self.0.marked(failure, marking);
        self.1.marked(failure, marking);
    }
}

fn core() -> Core {
    Core::new(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        crate::test_support::CAPACITY,
    )
    .expect("a valid capacity")
}

/// A rig whose core's observer also logs every call.
fn logged() -> (Rig, Arc<Log>) {
    let log = Arc::new(Log::default());
    let oracle = ConformanceOracle::new();
    let told = oracle.observer();
    let mut rig = Rig {
        core: core().observed_by(Arc::new(Both(told.clone(), log.clone()))),
        backend: MemoryBackend::new(nz(8), nz(16)),
        requests: Vec::new(),
        oracle,
        suspected: std::collections::HashMap::new(),
        told,
    };
    rig.attach(KEY, MemoryTarget::new());
    (rig, log)
}

/// Import a failure reaching every uncovered write.
fn import(rig: &mut Rig) {
    let (_, actions) = rig
        .core
        .import_failure(ImportScope::All, |_| {})
        .expect("room");
    rig.act(actions);
}

/// The next entry, which must be `Failed`: its token.
fn failed(rig: &mut Rig) -> FailureToken {
    match rig.pop_entry() {
        Some(Entry::Failed(failed)) => failed.token,
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn a_failure_and_its_suspects_are_told_in_push_order_before_its_failed_can_be_popped() {
    let (mut rig, log) = logged();
    rig.write(1, 0, b"a").expect("room");
    rig.write(2, 8, b"b").expect("room");
    rig.run();
    rig.entries();
    import(&mut rig);
    assert_eq!(
        log.take(),
        ["failed f0 imported", "suspect f0 op0", "suspect f0 op1"],
        "told while the Failed is still queued"
    );
    let _token = failed(&mut rig);
    assert!(log.take().is_empty(), "popping it tells nothing more");
}

#[test]
fn a_failure_suspecting_nothing_is_still_told_of() {
    let (mut rig, log) = logged();
    import(&mut rig);
    assert_eq!(log.take(), ["failed f0 imported"]);
    let _token = failed(&mut rig);
}

#[test]
fn a_marking_is_told_once_for_each_failure_in_the_order_observed() {
    let (mut rig, log) = logged();
    rig.write(1, 0, b"a").expect("room");
    rig.run();
    rig.entries();
    import(&mut rig);
    import(&mut rig);
    log.take();
    rig.seal(1);
    rig.run();
    assert_eq!(
        log.take(),
        ["marked f0 op0 covered", "marked f1 op0 covered"],
        "as made"
    );
    rig.entries();
}

#[test]
fn a_failure_resolved_before_a_marking_is_told_nothing_more() {
    let (mut rig, log) = logged();
    rig.write(1, 0, b"a").expect("room");
    rig.run();
    rig.entries();
    import(&mut rig);
    let first = failed(&mut rig);
    import(&mut rig);
    let _token = failed(&mut rig);
    let actions = rig
        .core
        .resolve(vec![(first, Resolution::Abandon)])
        .expect("a live token");
    rig.act(actions);
    log.take();
    rig.seal(1);
    rig.run();
    assert_eq!(log.take(), ["marked f1 op0 covered"]);
    rig.entries();
}

#[test]
#[should_panic(expected = "a failure observer was given to a core already in use")]
fn an_observer_given_after_an_operation_is_pushed_is_inconsistent() {
    let mut core = core();
    let mut backend = MemoryBackend::new(nz(8), nz(16));
    let target = MemoryTarget::new();
    core.add_target(KEY, target.describe()).expect("a new key");
    backend.attach(KEY, target).expect("a new key");
    let epoch = crate::types::Epoch::new(core.default_lineage(), 1);
    core.write(
        KEY,
        0,
        crate::backend::SubmissionData::Owned(vec![1]),
        epoch,
        "write",
        crate::types::WriteOptions::new(),
        &mut backend,
    )
    .expect("accepted");
    let _ = core.observed_by(Arc::new(Log::default()));
}

#[test]
#[should_panic(expected = "a failure observer was given twice")]
fn an_observer_given_twice_is_inconsistent() {
    let _ = core()
        .observed_by(Arc::new(Log::default()))
        .observed_by(Arc::new(Log::default()));
}

#[test]
#[should_panic(expected = "a failure observer was given to a core already in use")]
fn an_observer_given_after_a_failure_is_observed_is_inconsistent() {
    let mut core = core();
    let _ = core.import_failure(ImportScope::All, |_| {}).expect("room");
    let _ = core.observed_by(Arc::new(Log::default()));
}
