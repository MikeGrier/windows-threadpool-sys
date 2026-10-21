// Copyright (c) 2026 Mike Grier
//! Pool delivery over a real ring: completions drained on the thread pool, the driver told when
//! entries become poppable, a backlog reached at setup, a callback environment's pool, and the
//! engine handed back.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use crate::InterruptStamps;
use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::pool::ThreadpoolPool;

use super::PoolDelivery;
use crate::dioring::tests::TempFile;
use crate::held_pool::{Held, one_thread_pool};
use crate::windows_backend::{Win32FileTarget, WindowsBackend, WindowsBackendSetup};
use durable_ioring::DurableIoCore;
use durable_ioring::IoTarget;
use durable_ioring::NoDiagnostics;
use durable_ioring::backend::SubmissionData;
use durable_ioring::driver::{ENGINE_LOCK, Engine, LockWatch, lock};
use durable_ioring::oracle::ConformanceOracle;
use durable_ioring::{Diagnostic, DiagnosticKind, DiagnosticSink};
use durable_ioring::{DurabilityRequest, Entry, Epoch, IoTargetKey, OpKind, WriteOptions};

const KEY: IoTargetKey = IoTargetKey(1);
/// Generous: each wait is for real completions and a pool callback.
const BOUND: Duration = Duration::from_secs(5);
/// How long a test looks for a delivery that must not come.
const QUIET: Duration = Duration::from_millis(100);

type V = crate::WindowsIds<u64>;
type Eng = Engine<u64, WindowsBackend<Vec<u8>>, u32, InterruptStamps>;
type Pool = PoolDelivery<u64, Vec<u8>, u32, InterruptStamps, Vec<u8>>;

/// An engine over a fresh ring with `file` registered under `KEY`.
fn engine(file: &TempFile) -> Eng {
    let target = Win32FileTarget::new(file.open());
    let description = target.describe();
    let backend = WindowsBackend::new(WindowsBackendSetup {
        submission_queue_size: 16,
        completion_queue_size: 16,
        targets: vec![(KEY, target)],
        buffers: Vec::new(),
    })
    .expect("build the backend");
    let mut engine = Engine::new(
        DurableIoCore::new(InterruptStamps, Arc::new(NoDiagnostics)),
        backend,
    );
    engine.core.add_target(KEY, description).expect("a new key");
    engine
}

/// A pool delivery of `engine` that sends on every readiness.
fn delivered(engine: Eng, env: Option<&mut CallbackEnviron<'_>>) -> (Pool, Receiver<()>) {
    delivered_watched(
        engine,
        env,
        LockWatch::new(0, Arc::new(NoDiagnostics), Arc::new(InterruptStamps)),
    )
}

/// As [`delivered`], with the waits for the engine's lock reported through `watch`.
fn delivered_watched(
    engine: Eng,
    env: Option<&mut CallbackEnviron<'_>>,
    watch: LockWatch,
) -> (Pool, Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let pool = PoolDelivery::new(
        engine,
        move || {
            let _ = tx.send(());
        },
        env,
        watch,
    )
    .map_err(|(error, _)| error)
    .expect("wire the delivery");
    (pool, rx)
}

/// Push a write of `bytes` at epoch `id` through `engine`, telling the oracle.
fn write(engine: &mut Eng, oracle: &mut ConformanceOracle<V, u32>, id: u64, bytes: &[u8]) {
    let epoch = Epoch::new(engine.core.default_lineage(), id);
    let context = u32::try_from(id).expect("small");
    let op = engine
        .write(
            KEY,
            id * 16,
            SubmissionData::Owned(bytes.to_vec()),
            epoch,
            context,
            WriteOptions::new(),
        )
        .expect("accepted");
    let length = u32::try_from(bytes.len()).expect("small");
    oracle
        .pushed(op, OpKind::Write { epoch }, length, context)
        .expect("the oracle accepts the push");
}

/// Pop entries, waiting on readiness, until `done` says the last one is enough.
fn collect(
    pool: &Pool,
    ready: &Receiver<()>,
    oracle: &mut ConformanceOracle<V, u32>,
    done: impl Fn(&Entry<V, Vec<u8>, u32>) -> bool,
) -> Vec<Entry<V, Vec<u8>, u32>> {
    let started = Instant::now();
    let mut entries = Vec::new();
    loop {
        while let Some(entry) = pool.engine().core.pop() {
            oracle
                .observe(&entry)
                .expect("the oracle accepts the entry");
            let finished = done(&entry);
            entries.push(entry);
            if finished {
                return entries;
            }
        }
        let left = BOUND.saturating_sub(started.elapsed());
        assert!(
            ready.recv_timeout(left).is_ok(),
            "no readiness within {BOUND:?}; got {entries:?}"
        );
    }
}

fn durable(entry: &Entry<V, Vec<u8>, u32>) -> bool {
    matches!(entry, Entry::Durable { .. })
}

#[test]
fn writes_and_a_seal_are_delivered_on_the_pool() {
    let file = TempFile::new("pool-writes");
    let (pool, ready) = delivered(engine(&file), None);
    let mut oracle = ConformanceOracle::new();
    let through = {
        let mut engine = pool.engine();
        for id in 1..=3 {
            write(&mut engine, &mut oracle, id, b"pooled");
        }
        let through = Epoch::new(engine.core.default_lineage(), 3);
        let (answer, _) = engine.make_durable_through(through).expect("ours");
        assert_eq!(answer, DurabilityRequest::Submitted);
        through
    };
    oracle.sealed(through);
    let entries = collect(&pool, &ready, &mut oracle, durable);
    assert_eq!(
        entries.len(),
        4,
        "three completions, then durable: {entries:?}"
    );
    assert_eq!(
        pool.engine().core.durable_through(through.lineage),
        Ok(Some(3))
    );
    oracle.finish().expect("every operation completed");
}

#[test]
fn a_backlog_queued_before_delivery_is_reached_at_setup() {
    let file = TempFile::new("pool-backlog");
    let mut engine = engine(&file);
    let mut oracle = ConformanceOracle::new();
    write(&mut engine, &mut oracle, 1, b"early");
    // Given time to complete before any wake exists, so only the setup signal can reach it.
    std::thread::sleep(Duration::from_millis(50));
    let (pool, ready) = delivered(engine, None);
    let entries = collect(&pool, &ready, &mut oracle, |entry| {
        matches!(entry, Entry::Op(_))
    });
    assert_eq!(entries.len(), 1);
    oracle.finish().expect("every operation completed");
}

#[test]
fn a_callback_environment_binds_delivery_to_its_pool() {
    let file = TempFile::new("pool-env");
    let threads = ThreadpoolPool::new().expect("create a pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&threads);
    let (pool, ready) = delivered(engine(&file), Some(&mut env));
    let mut oracle = ConformanceOracle::new();
    let through = {
        let mut engine = pool.engine();
        write(&mut engine, &mut oracle, 1, b"bound");
        let through = Epoch::new(engine.core.default_lineage(), 1);
        engine.make_durable_through(through).expect("ours");
        through
    };
    oracle.sealed(through);
    collect(&pool, &ready, &mut oracle, durable);
    // The delivery goes first, then `env`, then the pool it names -- the reverse of declaration.
    drop(pool);
    oracle.finish().expect("every operation completed");
}

#[test]
fn delivery_waits_for_the_thread_of_the_pool_its_environment_names() {
    // Which pool ran a drain is not visible from the drain, so this holds the named pool's only
    // thread: a delivery bound to it must wait, and one on the default pool would not.
    let file = TempFile::new("pool-env-held");
    let threads = one_thread_pool();
    let mut env = CallbackEnviron::new();
    env.set_pool(&threads);
    let (pool, ready) = delivered(engine(&file), Some(&mut env));
    // Setup signals once as it attaches; take what it has told before the thread is held.
    while ready.recv_timeout(QUIET).is_ok() {}
    let held = Held::start(&mut env);
    let mut oracle = ConformanceOracle::new();
    write(&mut pool.engine(), &mut oracle, 1, b"held");
    assert!(
        ready.recv_timeout(QUIET).is_err(),
        "the named pool's only thread is held, so nothing was delivered"
    );
    held.release();
    collect(&pool, &ready, &mut oracle, |entry| {
        matches!(entry, Entry::Op(_))
    });
    oracle.finish().expect("every operation completed");
}

#[test]
fn readiness_is_told_on_each_transition_from_no_entries() {
    let file = TempFile::new("pool-ready");
    let (pool, ready) = delivered(engine(&file), None);
    let mut oracle = ConformanceOracle::new();
    for id in 1..=2 {
        write(&mut pool.engine(), &mut oracle, id, b"x");
        collect(&pool, &ready, &mut oracle, |entry| {
            matches!(entry, Entry::Op(_))
        });
    }
    oracle.finish().expect("every operation completed");
}

#[test]
fn the_engine_comes_back_once_delivery_stops() {
    let file = TempFile::new("pool-back");
    let (pool, ready) = delivered(engine(&file), None);
    let mut oracle = ConformanceOracle::new();
    write(&mut pool.engine(), &mut oracle, 1, b"x");
    collect(&pool, &ready, &mut oracle, |entry| {
        matches!(entry, Entry::Op(_))
    });
    let mut engine = pool.into_engine();
    write(&mut engine, &mut oracle, 2, b"y");
    // No pool drains it now: the engine is the caller's, and a drain by hand reaches it.
    let started = Instant::now();
    while engine.core.pop().is_none() {
        engine.drain();
        assert!(started.elapsed() < BOUND, "the write never completed");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Says when the test's own thread has found a lock held -- the sink is asked whether it wants the
/// wait as soon as there is one -- and keeps what is reported, with the thread that waited. A pool's
/// own thread drains once on the ring's setup signal and may wait on the same lock, which is why the
/// thread is part of the record.
struct Contended {
    test_thread: ThreadId,
    found_held: Mutex<Sender<()>>,
    waits: Mutex<Vec<(ThreadId, u64, String)>>,
}

impl DiagnosticSink for Contended {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        if kind == DiagnosticKind::LockWait && thread::current().id() == self.test_thread {
            let _ = self.found_held.lock().expect("the signal").send(());
        }
        kind == DiagnosticKind::LockWait
    }

    fn report(&self, event: &Diagnostic<'_>) {
        let Diagnostic::LockWait { instance, lock, .. } = event else {
            panic!("reported {event}");
        };
        self.waits.lock().expect("the record").push((
            thread::current().id(),
            *instance,
            (*lock).to_owned(),
        ));
    }
}

#[test]
fn a_consumer_call_that_waits_for_the_engine_reports_the_wait() {
    let file = TempFile::new("pool-lock-wait");
    let (found_held, signal) = mpsc::channel();
    let sink = Arc::new(Contended {
        test_thread: thread::current().id(),
        found_held: Mutex::new(found_held),
        waits: Mutex::default(),
    });
    let watch = LockWatch::new(5, sink.clone(), Arc::new(InterruptStamps));
    let (pool, _ready) = delivered_watched(engine(&file), None, watch);

    // Another thread holds the slot until this one's call has found it held.
    let slot = pool.shared();
    let (holding, held) = mpsc::channel();
    thread::scope(|scope| {
        let holder = scope.spawn(move || {
            let guard = lock(&slot);
            holding.send(()).expect("the test is listening");
            signal
                .recv_timeout(BOUND)
                .expect("the call found the lock held");
            drop(guard);
        });
        held.recv_timeout(BOUND).expect("the slot was taken");
        drop(pool.engine());
        holder.join().expect("the holder finished");
    });
    let waits = sink.waits.lock().expect("the record");
    let ours: Vec<_> = waits
        .iter()
        .filter(|(waiter, ..)| *waiter == thread::current().id())
        .collect();
    assert_eq!(ours.len(), 1, "{waits:?}");
    assert_eq!((ours[0].1, ours[0].2.as_str()), (5, ENGINE_LOCK));
    drop(waits);
    let _ = pool.into_engine();
}
