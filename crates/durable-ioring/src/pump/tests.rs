// Copyright (c) 2026 Mike Grier
//! Pumped delivery over the in-memory backend (DI-7.3): the wait made outside the engine's lock,
//! the drain to empty, and what a wait that fails or a notification that cannot be made leaves.
//!
//! The in-memory backend executes a submission when it is submitted and raises its notification
//! then, so a push alone raises nothing: a test that waits with no limit must have another thread
//! submit. A push through the engine is rung for by the engine's doorbell (DI-7+.2); the Windows
//! backend is `dioring/tests/pumped.rs`.

use std::io;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{PumpedDelivery, Waitable};
use crate::backend::{
    Completion, DuplicateTarget, IoBackend, RefusalCause, Refused, Submission, SubmissionData,
    SubmissionOp,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink, NoDiagnostics};
use crate::engine::{Engine, Slot, lock};
use crate::io_core::DurableIoCore;
use crate::io_target::IoTarget;
use crate::lock_watch::LockWatch;
use crate::memory::{MemoryBackend, MemoryNotification, MemoryTarget};
use crate::oracle::PairingChecker;
use crate::test_support::Ticks;
use crate::types::{Entry, Epoch, FlushDomain, IoFault, IoTargetKey, Outcome, WriteOptions};
use rc_errors::{ErrorReport, StaticError};

static WAIT_FAILED: StaticError = StaticError::new("the wait failed");
static NO_NOTIFICATION: StaticError = StaticError::new("no notification");

const ONE: IoTargetKey = IoTargetKey(1);
const TWO: IoTargetKey = IoTargetKey(2);

/// Generous: each wait is for another thread or for a completion that is already there.
const BOUND: Duration = Duration::from_secs(5);

type Pump = PumpedDelivery<u64, MemoryBackend, u32, Ticks>;
type ScriptedSlot = Slot<u64, Scripted, u32, Ticks>;

fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("non-zero")
}

/// An engine over two memory targets, with room for sixteen of each.
fn engine<Bk: IoBackend<Target = MemoryTarget, Buffer = Vec<u8>>>(
    backend: Bk,
) -> Engine<u64, Bk, u32, Ticks> {
    let core = DurableIoCore::new(Ticks::default(), Arc::new(NoDiagnostics));
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

fn memory() -> MemoryBackend {
    MemoryBackend::new(nz(16), nz(16))
}

/// Deliver `engine`, which must be able to make its notification.
fn delivered<Bk>(engine: Engine<u64, Bk, u32, Ticks>) -> PumpedDelivery<u64, Bk, u32, Ticks>
where
    Bk: IoBackend,
    Bk::Notification: Waitable,
{
    match PumpedDelivery::new(engine) {
        Ok(delivery) => delivery,
        Err((error, _)) => panic!("the notification could not be made: {error}"),
    }
}

fn pump() -> Pump {
    delivered(engine(memory()))
}

/// Push a write of `bytes` to `key` through `engine`, which runs it at the next submit.
fn push<Bk: IoBackend<Buffer = Vec<u8>>>(
    engine: &mut Engine<u64, Bk, u32, Ticks>,
    key: IoTargetKey,
    bytes: &[u8],
) {
    let epoch = Epoch::new(engine.core.default_lineage(), 1);
    engine
        .write(
            key,
            0,
            SubmissionData::Owned(bytes.to_vec()),
            epoch,
            7,
            WriteOptions::new(),
        )
        .expect("accepted");
}

/// Every entry now poppable, which must each be a completed write.
fn written<Bk: IoBackend<Buffer = Vec<u8>>>(engine: &mut Engine<u64, Bk, u32, Ticks>) -> Vec<u32> {
    std::iter::from_fn(|| engine.core.pop())
        .map(|entry| match entry {
            Entry::Op(completed) => match completed.outcome {
                Outcome::Transferred(n) => n,
                other => panic!("expected a transfer, got {other:?}"),
            },
            other => panic!("expected a completion, got {other:?}"),
        })
        .collect()
}

/// Whether `pumped`'s wait saw the notification raised.
fn raised(pumped: &super::Pumped) -> bool {
    *pumped.waited.as_ref().expect("the wait is answered")
}

#[test]
fn a_pump_drains_every_completion_at_once() {
    let mut pump = pump();
    {
        let mut engine = pump.engine();
        for i in 0..6_u8 {
            let key = if i % 2 == 0 { ONE } else { TWO };
            push(&mut engine, key, &vec![i; usize::from(i) + 1]);
        }
    }
    let pumped = pump.pump(Some(Duration::ZERO));
    assert!(pumped.ready, "completions became poppable");
    let mut engine = pump.engine();
    assert_eq!(
        written(&mut engine),
        vec![1, 2, 3, 4, 5, 6],
        "in push order"
    );
    assert_eq!(engine.backend.completed(), 0, "and the backend is empty");
    assert_eq!(engine.backend.queued(), 0);
}

#[test]
fn a_pump_with_nothing_completed_waits_its_timeout_and_reports_no_entries() {
    let mut pump = pump();
    let timeout = Duration::from_millis(60);
    let started = Instant::now();
    let pumped = pump.pump(Some(timeout));
    assert!(
        started.elapsed() >= timeout,
        "returned after {:?}",
        started.elapsed()
    );
    assert!(!pumped.ready);
    assert!(!raised(&pumped), "the wait timed out");
}

#[test]
fn a_zero_timeout_is_the_pump_that_does_not_wait() {
    let mut pump = pump();
    let started = Instant::now();
    let pumped = pump.pump(Some(Duration::ZERO));
    assert!(started.elapsed() < BOUND, "took {:?}", started.elapsed());
    assert!(!pumped.ready && !raised(&pumped));
    // The same pump drains what a push left: it did not wait, and it still drained.
    push(&mut pump.engine(), ONE, b"x");
    assert!(pump.pump(Some(Duration::ZERO)).ready);
}

#[test]
fn completions_already_in_the_backend_are_delivered_without_waiting() {
    let mut pump = pump();
    {
        let mut engine = pump.engine();
        push(&mut engine, ONE, b"abc");
        engine
            .backend
            .submit()
            .expect("the in-memory backend submits");
    }
    let started = Instant::now();
    let pumped = pump.pump(Some(BOUND));
    assert!(started.elapsed() < BOUND, "waited with work to take");
    assert!(pumped.ready);
    assert!(
        !raised(&pumped),
        "no wait was made: the first drain delivered"
    );
    assert_eq!(written(&mut pump.engine()), vec![3]);
    // The raising was not consumed: the next wait sees it, and finds nothing left.
    let spurious = pump.pump(Some(Duration::ZERO));
    assert!(raised(&spurious) && !spurious.ready);
}

#[test]
fn a_pump_whose_edge_was_taken_by_another_waiter_still_delivers_at_once() {
    // The notification here never reports a raising, as one whose edge another handle on the same
    // event had already taken. Waiting first would sleep out the whole timeout with the completion
    // sitting in the backend; draining first takes it.
    let mut backend = scripted(false, false);
    backend.starve = true;
    let mut pump = delivered(engine(backend));
    {
        let mut engine = pump.engine();
        push(&mut engine, ONE, b"edge");
        engine.backend.submit().expect("the backend submits");
    }
    let timeout = Duration::from_secs(3);
    let started = Instant::now();
    let pumped = pump.pump(Some(timeout));
    assert!(pumped.ready, "delivered");
    assert!(
        started.elapsed() < timeout / 2,
        "waited {:?} with work to take",
        started.elapsed()
    );
    assert_eq!(written(&mut pump.engine()), vec![4]);
}

#[test]
fn a_raising_an_earlier_drain_took_satisfies_one_more_wait_and_then_no_more() {
    let mut pump = pump();
    push(&mut pump.engine(), ONE, b"abc");
    assert!(
        pump.pump(Some(Duration::ZERO)).ready,
        "the drain submitted it and took it"
    );
    // The submit raised the notification after the wait: it is seen by the next one, with nothing
    // left to take -- a wake with nothing to pop is normal (the ring crate's D-19).
    let spurious = pump.pump(Some(Duration::ZERO));
    assert!(raised(&spurious) && !spurious.ready);
    let quiet = pump.pump(Some(Duration::from_millis(20)));
    assert!(!raised(&quiet) && !quiet.ready, "and then it has been seen");
}

#[test]
fn a_raising_from_another_thread_wakes_a_waiting_pump_with_no_limit() {
    let mut pump = pump();
    let shared = pump.shared();
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let mut slot = lock(&shared);
        let engine = slot.as_mut().expect("delivery has not stopped");
        push(engine, TWO, b"wake");
        engine
            .backend
            .submit()
            .expect("the in-memory backend submits");
    });
    // No limit: a timeout mistaken for none would return before the raising.
    let pumped = pump.pump(None);
    raiser.join().expect("the raiser");
    assert!(raised(&pumped));
    assert!(pumped.ready, "the completion was there to drain");
    assert_eq!(written(&mut pump.engine()), vec![4]);
}

#[test]
fn the_wait_is_made_without_the_engines_lock() {
    let mut pump = pump();
    let shared = pump.shared();
    let started = Instant::now();
    let waiter = std::thread::spawn(move || {
        let pumped = pump.pump(Some(BOUND));
        (pump, pumped)
    });
    // Another thread's push and submit, while the pump waits. Were the lock held through the wait,
    // this would block until the wait timed out, and the pump would report that.
    std::thread::sleep(Duration::from_millis(50));
    {
        let mut slot = lock(&shared);
        let engine = slot.as_mut().expect("delivery has not stopped");
        push(engine, ONE, b"unblocked");
        engine
            .backend
            .submit()
            .expect("the in-memory backend submits");
    }
    let (pump, pumped) = waiter.join().expect("the waiting pump");
    assert!(raised(&pumped), "woken by the raising, not timed out");
    assert!(started.elapsed() < BOUND, "took {:?}", started.elapsed());
    assert_eq!(written(&mut pump.engine()), vec![9]);
}

#[test]
fn the_engine_is_handed_back_and_its_slot_left_empty() {
    let pump = pump();
    let shared = pump.shared();
    let engine = pump.into_engine();
    assert!(
        lock(&shared).is_none(),
        "a path that still holds the slot finds it empty"
    );
    assert!(engine.backend.target(ONE).is_some() && engine.backend.target(TWO).is_some());
    assert!(engine.core.target(ONE).is_some());
}

#[test]
fn a_memory_notification_is_seen_once_for_however_many_raisings() {
    let backend = memory();
    let mut notification = backend.notification().expect("a notification");
    assert!(
        !notification.wait(Some(Duration::ZERO)).expect("a wait"),
        "nothing raised"
    );

    // Raised twice: a submit takes the completions from none to some, and once they are popped
    // another does again.
    let mut engine = engine(backend);
    push(&mut engine, ONE, b"a");
    engine.backend.submit().expect("submits");
    engine.backend.pop().expect("pops").expect("a completion");
    push(&mut engine, ONE, b"b");
    engine.backend.submit().expect("submits");
    assert_eq!(notification.raised(), 2);
    assert!(
        notification.wait(Some(BOUND)).expect("a wait"),
        "raised since the last wait"
    );
    assert!(
        !notification.wait(Some(Duration::ZERO)).expect("a wait"),
        "and the two raisings are one edge, seen once"
    );
}

#[test]
fn a_notification_cloned_after_a_wait_keeps_what_that_wait_saw() {
    let mut engine = engine(memory());
    let mut first = engine.backend.notification().expect("a notification");
    push(&mut engine, ONE, b"a");
    engine.backend.submit().expect("submits");
    assert!(first.wait(Some(BOUND)).expect("a wait"));
    let mut clone = first.clone();
    assert!(
        !clone.wait(Some(Duration::ZERO)).expect("a wait"),
        "it was seen"
    );
    let mut fresh: MemoryNotification = engine.backend.notification().expect("a notification");
    assert!(
        fresh.wait(Some(Duration::ZERO)).expect("a wait"),
        "a new one has seen nothing"
    );
}

/// The in-memory backend, with a notification that can be made to fail to be made, or to be
/// waited on.
struct Scripted {
    inner: MemoryBackend,
    notification_fails: bool,
    /// A fault the backend keeps a hold of, so the one it reports is shared and cannot be taken
    /// whole.
    kept_fault: Option<ErrorReport>,
    wait_fails: bool,
    /// A notification that never reports a raising, sleeping out its timeout instead.
    starve: bool,
    /// Run as a wait begins, as something completing while the pump waits.
    on_wait: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct ScriptedNotification {
    inner: MemoryNotification,
    fails: bool,
    starve: bool,
    on_wait: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Waitable for ScriptedNotification {
    fn wait(&mut self, timeout: Option<Duration>) -> Result<bool, IoFault> {
        if let Some(on_wait) = &self.on_wait {
            on_wait();
        }
        if self.fails {
            return Err(IoFault {
                source: "test",
                code: None,
                error: ErrorReport::from_static(&WAIT_FAILED),
            });
        }
        if self.starve {
            std::thread::sleep(timeout.unwrap_or(BOUND));
            return Ok(false);
        }
        self.inner.wait(timeout)
    }
}

impl IoBackend for Scripted {
    type Target = MemoryTarget;
    type Buffer = Vec<u8>;
    type Notification = ScriptedNotification;

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
        self.inner.submit()
    }

    fn pop(&mut self) -> Result<Option<Completion<Vec<u8>>>, IoFault> {
        self.inner.pop()
    }

    fn notification(&self) -> Result<ScriptedNotification, IoFault> {
        if self.notification_fails {
            let error = self
                .kept_fault
                .clone()
                .unwrap_or_else(|| ErrorReport::from_static(&NO_NOTIFICATION));
            return Err(IoFault {
                source: "test",
                code: None,
                error,
            });
        }
        Ok(ScriptedNotification {
            inner: self.inner.notification()?,
            fails: self.wait_fails,
            starve: self.starve,
            on_wait: self.on_wait.clone(),
        })
    }
}

fn scripted(notification_fails: bool, wait_fails: bool) -> Scripted {
    Scripted {
        inner: memory(),
        notification_fails,
        kept_fault: None,
        wait_fails,
        starve: false,
        on_wait: None,
    }
}

#[test]
fn a_notification_that_cannot_be_made_hands_the_engine_back() {
    let refused = PumpedDelivery::new(engine(scripted(true, false)));
    let Err((error, mut engine)) = refused else {
        panic!("the notification could not be made");
    };
    assert_eq!(
        error.source, "test",
        "the backend's fault, as it reported it"
    );
    assert_eq!(error.error, ErrorReport::from_static(&NO_NOTIFICATION));
    assert!(error.to_string().contains("no notification"), "{error}");
    assert!(
        engine.backend.inner.target(ONE).is_some() && engine.core.target(TWO).is_some(),
        "the engine comes back whole"
    );
    push(&mut engine, ONE, b"still works");
}

#[test]
fn a_notification_fault_the_backend_keeps_is_the_same_report_it_kept() {
    let mut backend = scripted(true, false);
    let kept = crate::report_io_error(io::Error::new(io::ErrorKind::NotFound, "a kept report"));
    backend.kept_fault = Some(kept.clone());
    let Err((error, engine)) = PumpedDelivery::new(engine(backend)) else {
        panic!("the notification could not be made");
    };
    assert_eq!(error.error, kept, "one report, shared, not copied");
    assert!(error.to_string().contains("a kept report"), "{error}");
    assert!(
        engine.backend.kept_fault.is_some(),
        "the engine still holds it"
    );
}

#[test]
fn a_wait_that_fails_still_drains_what_completed_during_it_and_reports_why() {
    // Something completes as the wait begins -- the hook pushes and submits through the delivery's
    // shared slot, which the pump does not hold while it waits -- and the wait then fails.
    let shared: Arc<Mutex<Option<ScriptedSlot>>> = Arc::default();
    let mut backend = scripted(false, true);
    let slot = Arc::clone(&shared);
    backend.on_wait = Some(Arc::new(move || {
        let slot = lock(&slot).clone().expect("the delivery's slot is set");
        let mut guard = lock(&slot);
        let engine = guard.as_mut().expect("delivery has not stopped");
        push(engine, ONE, b"during the wait");
        engine.backend.submit().expect("the backend submits");
    }));
    let mut pump = delivered(engine(backend));
    *lock(&shared) = Some(pump.shared());

    let pumped = pump.pump(Some(Duration::ZERO));
    assert!(pumped.ready, "what completed during the wait is delivered");
    let error = pumped.waited.expect_err("the wait failed");
    assert!(error.to_string().contains("the wait failed"), "{error}");
    assert_eq!(written(&mut pump.engine()), vec![15]);
}

/// Keeps the lock waits it is given, by their begins, holds them to the pairing rule, and says when
/// a wait is about to block: its begin is reported just before.
struct Waits {
    seen: Mutex<Vec<(u64, String)>>,
    checker: Mutex<PairingChecker>,
    about_to_block: std::sync::mpsc::Sender<()>,
}

impl Waits {
    fn new(about_to_block: std::sync::mpsc::Sender<()>) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::default(),
            checker: Mutex::default(),
            about_to_block,
        })
    }

    /// Every wait's pair ended, and the stream kept the rule.
    fn finished(&self) {
        let checker = std::mem::take(&mut *self.checker.lock().expect("the checker"));
        checker.finish(&[]).expect("every wait's pair ended");
    }
}

impl DiagnosticSink for Waits {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        kind == DiagnosticKind::LockWait
    }

    fn report(&self, event: &Diagnostic<'_>) {
        self.checker
            .lock()
            .expect("the checker")
            .observe(event)
            .expect("the pairing rule");
        match event {
            Diagnostic::LockWaitBegan { instance, lock, .. } => {
                self.seen
                    .lock()
                    .expect("the record")
                    .push((*instance, (*lock).to_owned()));
                self.about_to_block.send(()).expect("the test is listening");
            }
            Diagnostic::LockWaitEnded { .. } => {}
            other => panic!("reported {other}"),
        }
    }
}

#[test]
fn a_consumer_call_that_waits_for_the_engine_reports_the_wait_and_one_that_does_not_reports_none() {
    let (signal, about_to_block) = std::sync::mpsc::channel();
    let waits = Waits::new(signal);
    let watch = LockWatch::new(11, waits.clone());
    let pump = pump().with_lock_watch(watch);

    drop(pump.engine());
    assert!(waits.seen.lock().expect("the record").is_empty());

    let slot = pump.shared();
    let held = lock(&slot);
    std::thread::scope(|scope| {
        let other = scope.spawn(|| drop(pump.engine()));
        about_to_block
            .recv()
            .expect("the other call reached its wait");
        drop(held);
        other.join().expect("the other call finished");
    });
    assert_eq!(
        *waits.seen.lock().expect("the record"),
        [(11, "engine".to_owned())]
    );
    waits.finished();
}

#[test]
fn a_pumps_drain_that_waits_for_the_engine_reports_the_wait() {
    let (signal, about_to_block) = std::sync::mpsc::channel();
    let waits = Waits::new(signal);
    let watch = LockWatch::new(12, waits.clone());
    let mut pump = pump().with_lock_watch(watch);

    let slot = pump.shared();
    let held = lock(&slot);
    std::thread::scope(|scope| {
        let other = scope.spawn(|| pump.pump(Some(Duration::ZERO)));
        about_to_block.recv().expect("the drain reached its wait");
        drop(held);
        other.join().expect("the pump finished");
    });
    assert_eq!(
        *waits.seen.lock().expect("the record"),
        [(12, "engine".to_owned())]
    );
    waits.finished();
}
