// Copyright (c) 2026 Mike Grier
//! The delay events through the driver (DI-12.1.1, DI-D-26): a wait for the engine's lock made by a
//! path of the instance's that is not the consumer's own -- the hub a lineage handle releases
//! through -- is reported to the instance's sink, as the consumer's own calls' are
//! (`windows_backend/pool/tests.rs`).

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

use super::{Ring, setup};
use crate::dioring::{DeliveryMode, Setup};
use durable_ioring::driver::{ENGINE_LOCK, Home, lock};
use durable_ioring::{Diagnostic, DiagnosticKind, DiagnosticSink, Lineage};

const BOUND: Duration = Duration::from_secs(5);

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

fn watched(delivery: DeliveryMode<'static, 'static>) -> (Ring, Arc<Contended>, mpsc::Receiver<()>) {
    let (found_held, signal) = mpsc::channel();
    let sink = Arc::new(Contended {
        test_thread: thread::current().id(),
        found_held: Mutex::new(found_held),
        waits: Mutex::default(),
    });
    let ring = Ring::new(Setup {
        delivery,
        diagnostics: Some(sink.clone()),
        ..setup(Vec::new(), Vec::new(), None)
    })
    .expect("build an instance")
    .0;
    (ring, sink, signal)
}

/// Run `act`, here, while another thread holds the engine's slot until `act` has found it held, and
/// check that the wait was reported by this thread, once, as a wait for the engine's lock.
fn waits_reported(delivery: DeliveryMode<'static, 'static>, act: impl Fn(&Ring, Lineage)) {
    let (ring, sink, signal) = watched(delivery);
    let lineage = ring.default.lineage();
    let instance = lineage.instance.number();
    let slot = ring.delivery.as_ref().expect("live").shared();
    let (holding, held) = mpsc::channel();
    thread::scope(|scope| {
        let holder = scope.spawn(move || {
            let guard = lock(&slot);
            holding.send(()).expect("the test is listening");
            signal
                .recv_timeout(BOUND)
                .expect("the call found the engine held");
            drop(guard);
        });
        held.recv_timeout(BOUND).expect("the slot was taken");
        act(&ring, lineage);
        holder.join().expect("the holder finished");
    });
    let waits = sink.waits.lock().expect("the record");
    let ours: Vec<_> = waits
        .iter()
        .filter(|(waiter, ..)| *waiter == thread::current().id())
        .collect();
    assert_eq!(ours.len(), 1, "{waits:?}");
    assert_eq!((ours[0].1, ours[0].2.as_str()), (instance, ENGINE_LOCK));
}

fn each_delivery(act: impl Fn(&Ring, Lineage)) {
    waits_reported(DeliveryMode::Pool(None), &act);
    waits_reported(DeliveryMode::Pumped, &act);
    waits_reported(DeliveryMode::Signalled(None), &act);
}

#[test]
fn a_release_that_waits_for_the_engine_is_reported_as_a_wait_for_the_engines_lock() {
    each_delivery(|ring, lineage| ring.hub.release(lineage));
}

#[test]
fn a_consumers_call_that_waits_for_the_engine_is_reported_as_a_wait_for_the_engines_lock() {
    each_delivery(|ring, _| drop(ring.engine()));
}
