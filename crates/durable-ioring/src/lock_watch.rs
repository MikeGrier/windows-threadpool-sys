// Copyright (c) 2026 Mike Grier
//! The waits for an instance's locks (DI-12.1.1, DI-D-26, DI-D-87): a lock taken through a
//! [`LockWatch`] reports a wait when -- and only when -- taking it had to: a pair, its begin when
//! the immediate try fails and its end as the lock is taken, with the lock held; and, until the
//! sinks have moved to the pair (DI-19.2.4), a [`Diagnostic::LockWait`] after the lock is let go.
//!
//! The core's sink and clock live inside the engine, which is itself behind the lock being timed,
//! so a wait cannot be timed from inside it. The watch holds its own handles to both, and sits
//! beside the lock instead.
//!
//! An acquisition that is immediate costs a `try_lock` and nothing else: no clock is read and no
//! event is built. The clock is read only once the lock is found held, and only if the sink wants
//! the kind.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::Duration;

use crate::clock::StampClock;
use crate::correlation::CorrelationId;
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink};

/// What an instance calls the lock on its engine's slot in a [`LockWatch`] report.
pub const ENGINE_LOCK: &str = "engine";

/// A clock that times a blocking call, so a [`LockWatch`] need not be generic over the clock's
/// reading.
pub trait LockTimer: Send + Sync {
    /// Run `blocking`, and say how long it took.
    fn time(&self, blocking: &mut dyn FnMut()) -> Duration;
}

impl<K: StampClock + Sync> LockTimer for K {
    fn time(&self, blocking: &mut dyn FnMut()) -> Duration {
        let start = self.now();
        blocking();
        self.elapsed(start, self.now())
    }
}

/// Where an instance's lock waits are reported, and by what clock they are timed.
#[derive(Clone)]
pub struct LockWatch {
    instance: u64,
    sink: Arc<dyn DiagnosticSink>,
    timer: Arc<dyn LockTimer>,
}

impl LockWatch {
    /// A watch reporting `instance`'s lock waits to `sink`, timed by `timer`.
    pub fn new(instance: u64, sink: Arc<dyn DiagnosticSink>, timer: Arc<dyn LockTimer>) -> Self {
        Self {
            instance,
            sink,
            timer,
        }
    }

    /// Lock `mutex`, which this instance calls `name`. A panic while it was held is an
    /// inconsistency the core already reported and panicked on (DI-D-46): the state is left as
    /// that panic found it, as [`crate::engine::lock`] does.
    pub fn lock<'a, T>(&'a self, mutex: &'a Mutex<T>, name: &'static str) -> WatchedGuard<'a, T> {
        let (guard, waited) = match mutex.try_lock() {
            Ok(guard) => (guard, None),
            Err(TryLockError::Poisoned(poisoned)) => (poisoned.into_inner(), None),
            Err(TryLockError::WouldBlock) => {
                // One question decides both the pair and the wait reported after: they share a kind.
                let Some(id) = CorrelationId::for_pair(&*self.sink, DiagnosticKind::LockWait)
                else {
                    return WatchedGuard::unwatched(
                        mutex.lock().unwrap_or_else(PoisonError::into_inner),
                    );
                };
                self.sink.report(&Diagnostic::LockWaitBegan {
                    instance: self.instance,
                    lock: name,
                    id,
                });
                let mut taken = None;
                let waited = self.timer.time(&mut || {
                    taken = Some(mutex.lock().unwrap_or_else(PoisonError::into_inner));
                });
                // Written with the lock held, as the engineer ruled (DI-D-80): the sink's own
                // synchronization is a leaf beneath it, which `DiagnosticSink` requires.
                self.sink.report(&Diagnostic::LockWaitEnded {
                    instance: self.instance,
                    lock: name,
                    id,
                });
                (taken.expect("the lock was taken"), Some(waited))
            }
        };
        WatchedGuard {
            guard: Some(guard),
            wait: waited.map(|waited| Wait {
                watch: self,
                name,
                waited,
            }),
        }
    }

    fn report(&self, name: &'static str, waited: Duration) {
        self.sink.report(&Diagnostic::LockWait {
            instance: self.instance,
            lock: name,
            waited,
        });
    }
}

/// The wait to report once the lock is let go.
struct Wait<'a> {
    watch: &'a LockWatch,
    name: &'static str,
    waited: Duration,
}

/// A locked mutex whose acquisition may have waited: let go, then reported, so the report does not
/// lengthen the hold it measures.
pub struct WatchedGuard<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
    wait: Option<Wait<'a>>,
}

impl<'a, T> WatchedGuard<'a, T> {
    /// A guard whose acquisition is not reported: a lock taken with no watch.
    pub fn unwatched(guard: MutexGuard<'a, T>) -> Self {
        Self {
            guard: Some(guard),
            wait: None,
        }
    }
}

impl<T> Deref for WatchedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.guard.as_deref().expect("held until dropped")
    }
}

impl<T> DerefMut for WatchedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_deref_mut().expect("held until dropped")
    }
}

impl<T> Drop for WatchedGuard<'_, T> {
    fn drop(&mut self) {
        drop(self.guard.take());
        if let Some(wait) = self.wait.take() {
            wait.watch.report(wait.name, wait.waited);
        }
    }
}

#[cfg(test)]
mod tests;
