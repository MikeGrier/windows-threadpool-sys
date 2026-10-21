// Copyright (c) 2026 Mike Grier
//! The lock watch (DI-12.1.1, DI-19.2.3): an immediate acquisition is free and silent; a waited one
//! is a pair, its begin before the wait and its end with the lock held, and is timed and reported
//! after the lock is let go.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::{LockTimer, LockWatch, WatchedGuard};
use crate::clock::StampClock;
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink};
use crate::oracle::PairingChecker;
use crate::test_support::{ManualClock, Ticks};

type Hook = Box<dyn Fn() + Send>;

/// A sink keeping each lock wait it is given, and wanting them or not as it was built.
struct Waits {
    wanted: bool,
    seen: Mutex<Vec<(u64, &'static str, Duration)>>,
    /// Run on each report of a wait after the lock was let go, for a test that looks at the world
    /// as the report finds it.
    on_report: Mutex<Option<Hook>>,
    /// The pairs' lines, and the pairing rule over them.
    pairs: Mutex<(Vec<String>, PairingChecker)>,
    /// Run as a pair's end is reported, for a test that looks at the world as the end finds it.
    on_end: Mutex<Option<Hook>>,
}

impl Waits {
    fn new(wanted: bool) -> Arc<Self> {
        Arc::new(Self {
            wanted,
            seen: Mutex::new(Vec::new()),
            on_report: Mutex::new(None),
            pairs: Mutex::new((Vec::new(), PairingChecker::new())),
            on_end: Mutex::new(None),
        })
    }

    fn pair_lines(&self) -> Vec<String> {
        self.pairs.lock().expect("the pairs").0.clone()
    }

    /// Every pair begun has ended, and the stream kept the rule.
    fn pairs_finished(&self) {
        let checker = std::mem::take(&mut self.pairs.lock().expect("the pairs").1);
        checker.finish(&[]).expect("every wait's pair ended");
    }

    fn seen(&self) -> Vec<(u64, &'static str, Duration)> {
        self.seen.lock().expect("the record").clone()
    }
}

impl DiagnosticSink for Waits {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        self.wanted && kind == DiagnosticKind::LockWait
    }

    fn report(&self, event: &Diagnostic<'_>) {
        if event.pairing().is_some() {
            if matches!(event, Diagnostic::LockWaitEnded { .. })
                && let Some(look) = &*self.on_end.lock().expect("the hook")
            {
                look();
            }
            let mut pairs = self.pairs.lock().expect("the pairs");
            pairs.0.push(event.to_string());
            pairs.1.observe(event).expect("the pairing rule");
            return;
        }
        let Diagnostic::LockWait {
            instance,
            lock,
            waited,
        } = event
        else {
            panic!("a lock watch reported {event}");
        };
        if let Some(look) = &*self.on_report.lock().expect("the hook") {
            look();
        }
        self.seen
            .lock()
            .expect("the record")
            .push((*instance, lock, *waited));
    }
}

/// A timer that says it took `took`, counts its uses, and tells a test when it is about to block.
struct Gate {
    took: Duration,
    uses: AtomicU32,
    about_to_block: Mutex<Option<Sender<()>>>,
}

impl Gate {
    fn taking(took: Duration) -> Arc<Self> {
        Arc::new(Self {
            took,
            uses: AtomicU32::new(0),
            about_to_block: Mutex::new(None),
        })
    }

    fn uses(&self) -> u32 {
        self.uses.load(Ordering::Relaxed)
    }
}

impl LockTimer for Gate {
    fn time(&self, blocking: &mut dyn FnMut()) -> Duration {
        self.uses.fetch_add(1, Ordering::Relaxed);
        if let Some(signal) = &*self.about_to_block.lock().expect("the gate") {
            signal.send(()).expect("the test is listening");
        }
        blocking();
        self.took
    }
}

fn watch(instance: u64, sink: &Arc<Waits>, timer: Arc<dyn LockTimer>) -> LockWatch {
    LockWatch::new(instance, sink.clone(), timer)
}

/// Take `name` through `watch` on another thread while this one holds `mutex`, and let go of it
/// once the other has found it held.
fn waited_for<T: Send>(watch: &LockWatch, gate: &Gate, mutex: &Mutex<T>, name: &'static str) {
    let (signal, found_held) = channel();
    *gate.about_to_block.lock().expect("the gate") = Some(signal);
    let held = mutex.lock().expect("not poisoned");
    thread::scope(|scope| {
        let other = scope.spawn(|| drop(watch.lock(mutex, name)));
        found_held
            .recv()
            .expect("the other thread reached its wait");
        drop(held);
        other.join().expect("the other thread finished");
    });
}

#[test]
fn an_immediate_acquisition_reports_nothing_and_never_uses_the_timer() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_secs(1)));
    let mutex = Mutex::new(5);
    let watch = watch(1, &sink, gate.clone());
    assert_eq!(*watch.lock(&mutex, "engine"), 5);
    assert!(sink.seen().is_empty());
    assert_eq!(gate.uses(), 0);
}

#[test]
fn an_immediate_acquisition_reads_no_clock() {
    let (sink, clock) = (Waits::new(true), Ticks::default());
    let mutex = Mutex::new(());
    let watch = LockWatch::new(1, sink, Arc::new(clock.clone()));
    drop(watch.lock(&mutex, "engine"));
    drop(watch.lock(&mutex, "engine"));
    assert_eq!(clock.now(), 0, "the clock was read though no wait happened");
}

#[test]
fn a_waited_acquisition_reports_its_instance_its_lock_and_how_long() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_micros(40)));
    let mutex = Mutex::new(());
    let watch = watch(9, &sink, gate.clone());
    waited_for(&watch, &gate, &mutex, "engine");
    assert_eq!(sink.seen(), [(9, "engine", Duration::from_micros(40))]);
    assert_eq!(gate.uses(), 1);
}

#[test]
fn each_waited_acquisition_is_reported_once() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_nanos(3)));
    let mutex = Mutex::new(());
    let watch = watch(2, &sink, gate.clone());
    waited_for(&watch, &gate, &mutex, "engine");
    waited_for(&watch, &gate, &mutex, "hub");
    assert_eq!(
        sink.seen(),
        [
            (2, "engine", Duration::from_nanos(3)),
            (2, "hub", Duration::from_nanos(3))
        ]
    );
}

#[test]
fn the_wait_is_reported_after_the_lock_is_let_go() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_nanos(1)));
    let mutex = Arc::new(Mutex::new(()));
    let looked = Arc::new(AtomicU32::new(0));
    {
        let (mutex, looked) = (mutex.clone(), looked.clone());
        *sink.on_report.lock().expect("the hook") = Some(Box::new(move || {
            assert!(
                mutex.try_lock().is_ok(),
                "reported with the lock still held"
            );
            looked.fetch_add(1, Ordering::Relaxed);
        }));
    }
    let watch = watch(1, &sink, gate.clone());
    waited_for(&watch, &gate, &mutex, "engine");
    assert_eq!(looked.load(Ordering::Relaxed), 1);
}

#[test]
fn a_waited_acquisition_by_a_sink_that_does_not_want_it_is_not_timed() {
    let (sink, gate) = (Waits::new(false), Gate::taking(Duration::from_secs(1)));
    let mutex = Mutex::new(());
    let watch = watch(1, &sink, gate.clone());
    let (signal, started) = channel();
    let held = mutex.lock().expect("not poisoned");
    thread::scope(|scope| {
        let other = scope.spawn(|| {
            signal.send(()).expect("the test is listening");
            drop(watch.lock(&mutex, "engine"));
        });
        started.recv().expect("the other thread started");
        thread::sleep(Duration::from_millis(20));
        drop(held);
        other.join().expect("the other thread finished");
    });
    assert!(sink.seen().is_empty());
    assert_eq!(gate.uses(), 0, "timed though the kind is not wanted");
}

#[test]
fn a_poisoned_lock_is_taken_as_the_panic_left_it_and_reports_nothing() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_secs(1)));
    let mutex = Mutex::new(7);
    let poisoned = thread::scope(|scope| {
        scope
            .spawn(|| {
                let _held = mutex.lock().expect("not yet poisoned");
                panic!("poisoning the lock on purpose");
            })
            .join()
    });
    assert!(poisoned.is_err());
    assert!(mutex.is_poisoned());
    let watch = watch(1, &sink, gate.clone());
    assert_eq!(*watch.lock(&mutex, "engine"), 7);
    assert!(sink.seen().is_empty());
    assert_eq!(gate.uses(), 0);
}

#[test]
fn a_change_made_through_the_guard_is_there_when_it_is_let_go() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::ZERO));
    let mutex = Mutex::new(vec![1]);
    let watch = watch(1, &sink, gate);
    watch.lock(&mutex, "engine").push(2);
    assert_eq!(*mutex.lock().expect("not poisoned"), [1, 2]);
}

#[test]
fn an_unwatched_guard_derefs_and_reports_nothing() {
    let mutex = Mutex::new(1);
    let mut guard = WatchedGuard::unwatched(mutex.lock().expect("not poisoned"));
    *guard += 1;
    assert_eq!(*guard, 2);
    drop(guard);
    assert_eq!(*mutex.lock().expect("not poisoned"), 2);
}

#[test]
fn a_clock_times_the_call_it_is_given_in_its_own_unit() {
    let clock = ManualClock::at(100);
    let took = LockTimer::time(&clock, &mut || clock.advance(250));
    assert_eq!(took, Duration::from_nanos(250));
}

#[test]
fn a_clock_that_did_not_move_times_a_call_as_no_time() {
    let clock = ManualClock::at(100);
    assert_eq!(LockTimer::time(&clock, &mut || {}), Duration::ZERO);
}

#[test]
fn the_report_reads_as_a_sentence_naming_the_instance_and_the_lock() {
    let event = Diagnostic::LockWait {
        instance: 4,
        lock: "engine",
        waited: Duration::from_nanos(12),
    };
    assert_eq!(
        event.to_string(),
        "instance 4 waited 12ns for its engine lock"
    );
}

#[test]
fn a_waited_acquisition_is_one_pair_naming_its_instance_and_its_lock() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_nanos(1)));
    let mutex = Mutex::new(());
    let watch = watch(9, &sink, gate.clone());
    waited_for(&watch, &gate, &mutex, "engine");
    let lines = sink.pair_lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with("instance 9 (pair ")
            && lines[0].ends_with("waits for its engine lock"),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("instance 9 (pair ") && lines[1].ends_with("took its engine lock"),
        "{lines:?}"
    );
    sink.pairs_finished();
}

#[test]
fn a_pairs_begin_is_written_before_the_wait_and_its_end_after_it() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_nanos(1)));
    let mutex = Mutex::new(());
    let watch = watch(1, &sink, gate.clone());
    let (signal, found_held) = channel();
    *gate.about_to_block.lock().expect("the gate") = Some(signal);
    let held = mutex.lock().expect("not poisoned");
    thread::scope(|scope| {
        let other = scope.spawn(|| drop(watch.lock(&mutex, "engine")));
        found_held
            .recv()
            .expect("the other thread reached its wait");
        assert_eq!(
            sink.pair_lines().len(),
            1,
            "the begin, and only the begin, before the wait"
        );
        drop(held);
        other.join().expect("the other thread finished");
    });
    assert_eq!(sink.pair_lines().len(), 2);
    sink.pairs_finished();
}

#[test]
fn a_pairs_end_is_written_with_the_lock_held_and_the_sinks_own_lock_is_a_leaf_beneath_it() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_nanos(1)));
    let mutex = Arc::new(Mutex::new(()));
    let looked = Arc::new(AtomicU32::new(0));
    {
        let (mutex, looked) = (mutex.clone(), looked.clone());
        *sink.on_end.lock().expect("the hook") = Some(Box::new(move || {
            assert!(
                mutex.try_lock().is_err(),
                "the end was written with the lock let go"
            );
            looked.fetch_add(1, Ordering::Relaxed);
        }));
    }
    let watch = watch(1, &sink, gate.clone());
    waited_for(&watch, &gate, &mutex, "engine");
    assert_eq!(looked.load(Ordering::Relaxed), 1);
    // The sink took its own mutex to record the end while the watched lock was held, and no
    // thread holds the sink's mutex while it takes the watched one: the order is a leaf beneath.
    assert_eq!(sink.pair_lines().len(), 2);
    sink.pairs_finished();
}

#[test]
fn each_waited_acquisition_is_its_own_pair() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_nanos(3)));
    let mutex = Mutex::new(());
    let watch = watch(2, &sink, gate.clone());
    for name in ["engine", "hub", "engine", "slot"] {
        waited_for(&watch, &gate, &mutex, name);
    }
    let lines = sink.pair_lines();
    assert_eq!(lines.len(), 8, "{lines:?}");
    sink.pairs_finished();
}

#[test]
fn an_immediate_acquisition_is_no_pair() {
    let (sink, gate) = (Waits::new(true), Gate::taking(Duration::from_secs(1)));
    let mutex = Mutex::new(());
    let watch = watch(1, &sink, gate);
    drop(watch.lock(&mutex, "engine"));
    assert!(sink.pair_lines().is_empty());
    sink.pairs_finished();
}

#[test]
fn a_waited_acquisition_by_a_sink_that_does_not_want_it_is_no_pair() {
    let (sink, gate) = (Waits::new(false), Gate::taking(Duration::from_secs(1)));
    let mutex = Mutex::new(());
    let watch = watch(1, &sink, gate);
    let (signal, started) = channel();
    let held = mutex.lock().expect("not poisoned");
    thread::scope(|scope| {
        let other = scope.spawn(|| {
            signal.send(()).expect("the test is listening");
            drop(watch.lock(&mutex, "engine"));
        });
        started.recv().expect("the other thread started");
        thread::sleep(Duration::from_millis(20));
        drop(held);
        other.join().expect("the other thread finished");
    });
    assert!(sink.pair_lines().is_empty());
}
