// Copyright (c) 2026 Mike Grier
//! The lock watch (DI-12.1.1, DI-19.2.3): an immediate acquisition is free and silent; a waited one
//! is a pair, its begin before the wait and its end with the lock held, held to the pairing rule by
//! `PairingChecker`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use super::{LockWatch, WatchedGuard};
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink};
use crate::oracle::PairingChecker;

type Hook = Box<dyn Fn() + Send>;

/// A sink keeping each lock wait's pair, checked against the pairing rule, and wanting lock waits
/// or not as it was built. It records under a mutex it holds only for that.
struct Waits {
    wanted: bool,
    asked: AtomicU32,
    pairs: Mutex<(Vec<String>, PairingChecker)>,
    /// Told as a begin is reported: the waiting thread is about to block.
    began: Mutex<Option<Sender<()>>>,
    /// Run as an end is reported, for a test that looks at the world as the end finds it.
    on_end: Mutex<Option<Hook>>,
}

impl Waits {
    fn new(wanted: bool) -> Arc<Self> {
        Arc::new(Self {
            wanted,
            asked: AtomicU32::new(0),
            pairs: Mutex::new((Vec::new(), PairingChecker::new())),
            began: Mutex::new(None),
            on_end: Mutex::new(None),
        })
    }

    fn lines(&self) -> Vec<String> {
        self.pairs.lock().expect("the pairs").0.clone()
    }

    /// Every pair begun has ended, and the stream kept the rule.
    fn finished(&self) {
        let checker = std::mem::take(&mut self.pairs.lock().expect("the pairs").1);
        checker.finish(&[]).expect("every wait's pair ended");
    }
}

impl DiagnosticSink for Waits {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        self.asked.fetch_add(1, Ordering::Relaxed);
        self.wanted && kind == DiagnosticKind::LockWait
    }

    fn report(&self, event: &Diagnostic<'_>) {
        assert!(event.pairing().is_some(), "a lock watch reported {event}");
        if matches!(event, Diagnostic::LockWaitEnded { .. })
            && let Some(look) = &*self.on_end.lock().expect("the hook")
        {
            look();
        }
        {
            let mut pairs = self.pairs.lock().expect("the pairs");
            pairs.0.push(event.to_string());
            pairs.1.observe(event).expect("the pairing rule");
        }
        if matches!(event, Diagnostic::LockWaitBegan { .. })
            && let Some(began) = &*self.began.lock().expect("the signal")
        {
            began.send(()).expect("the test is listening");
        }
    }
}

fn watch(instance: u64, sink: &Arc<Waits>) -> LockWatch {
    LockWatch::new(instance, sink.clone())
}

/// Take `name` through `watch` on another thread while this one holds `mutex`, and let go of it
/// once the other has reported its wait's begin; `meanwhile` runs while it waits.
fn waited_for<T: Send>(
    watch: &LockWatch,
    sink: &Waits,
    mutex: &Mutex<T>,
    name: &'static str,
    meanwhile: impl FnOnce(),
) {
    let (signal, began) = channel();
    *sink.began.lock().expect("the signal") = Some(signal);
    let held = mutex.lock().expect("not poisoned");
    thread::scope(|scope| {
        let other = scope.spawn(|| drop(watch.lock(mutex, name)));
        began.recv().expect("the other thread began its wait");
        meanwhile();
        drop(held);
        other.join().expect("the other thread finished");
    });
}

#[test]
fn an_immediate_acquisition_reports_nothing_and_asks_nothing() {
    let sink = Waits::new(true);
    let mutex = Mutex::new(5);
    let watch = watch(1, &sink);
    assert_eq!(*watch.lock(&mutex, "engine"), 5);
    drop(watch.lock(&mutex, "engine"));
    assert!(sink.lines().is_empty());
    assert_eq!(
        sink.asked.load(Ordering::Relaxed),
        0,
        "the sink was asked though no wait happened"
    );
    sink.finished();
}

#[test]
fn a_waited_acquisition_is_one_pair_naming_its_instance_and_its_lock() {
    let sink = Waits::new(true);
    let mutex = Mutex::new(());
    let watch = watch(9, &sink);
    waited_for(&watch, &sink, &mutex, "engine", || {});
    let lines = sink.lines();
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
    assert_eq!(sink.asked.load(Ordering::Relaxed), 1, "asked once");
    sink.finished();
}

#[test]
fn a_pairs_begin_is_written_before_the_wait_and_its_end_after_it() {
    let sink = Waits::new(true);
    let mutex = Mutex::new(());
    let watch = watch(1, &sink);
    waited_for(&watch, &sink, &mutex, "engine", || {
        assert_eq!(
            sink.lines().len(),
            1,
            "the begin, and only the begin, before the wait"
        );
    });
    assert_eq!(sink.lines().len(), 2);
    sink.finished();
}

#[test]
fn a_pairs_end_is_written_with_the_lock_held_and_the_sinks_own_lock_is_a_leaf_beneath_it() {
    let sink = Waits::new(true);
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
    let watch = watch(1, &sink);
    waited_for(&watch, &sink, &mutex, "engine", || {});
    assert_eq!(looked.load(Ordering::Relaxed), 1);
    // The sink took its own mutex to record the end while the watched lock was held, and no
    // thread holds the sink's mutex while it takes the watched one: the order is a leaf beneath.
    sink.finished();
}

#[test]
fn each_waited_acquisition_is_its_own_pair() {
    let sink = Waits::new(true);
    let mutex = Mutex::new(());
    let watch = watch(2, &sink);
    for name in ["engine", "hub", "engine", "slot"] {
        waited_for(&watch, &sink, &mutex, name, || {});
    }
    assert_eq!(sink.lines().len(), 8);
    sink.finished();
}

#[test]
fn a_waited_acquisition_by_a_sink_that_does_not_want_it_is_no_pair_and_asks_once() {
    let sink = Waits::new(false);
    let mutex = Mutex::new(());
    let watch = watch(1, &sink);
    let (signal, started) = channel();
    let held = mutex.lock().expect("not poisoned");
    thread::scope(|scope| {
        let other = scope.spawn(|| {
            signal.send(()).expect("the test is listening");
            drop(watch.lock(&mutex, "engine"));
        });
        started.recv().expect("the other thread started");
        thread::sleep(std::time::Duration::from_millis(20));
        drop(held);
        other.join().expect("the other thread finished");
    });
    assert!(sink.lines().is_empty());
    assert!(
        sink.asked.load(Ordering::Relaxed) <= 1,
        "asked once at most"
    );
    sink.finished();
}

#[test]
fn a_poisoned_lock_is_taken_as_the_panic_left_it_and_reports_nothing() {
    let sink = Waits::new(true);
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
    let watch = watch(1, &sink);
    assert_eq!(*watch.lock(&mutex, "engine"), 7);
    assert!(sink.lines().is_empty());
    sink.finished();
}

#[test]
fn a_change_made_through_the_guard_is_there_when_it_is_let_go() {
    let sink = Waits::new(true);
    let mutex = Mutex::new(vec![1]);
    let watch = watch(1, &sink);
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
