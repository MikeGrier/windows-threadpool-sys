// Copyright (c) 2026 Mike Grier
//! The waits for an instance's locks (DI-12.1.1, DI-D-26, DI-D-87): a lock taken through a
//! [`LockWatch`] reports a wait when -- and only when -- taking it had to, as a pair: its begin
//! when the immediate try fails, and its end as the lock is taken, with the lock held.
//!
//! The core's sink lives inside the engine, which is itself behind the lock being watched, so a wait
//! cannot be reported from inside it. The watch holds its own handle to the sink, and sits beside
//! the lock instead.
//!
//! An acquisition that is immediate costs a `try_lock` and nothing else: no id is minted and no
//! event is built. A sink is asked once a lock is found held, and only then.

use alloc::sync::Arc;
use core::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

use crate::alloc_tags::{tag, tagged};
use crate::correlation::CorrelationId;
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink};

/// What an instance calls the lock on its engine's slot in a [`LockWatch`] report.
pub const ENGINE_LOCK: &str = "engine";

/// Where an instance's lock waits are reported.
#[derive(Clone)]
pub struct LockWatch {
    instance: u64,
    sink: Arc<dyn DiagnosticSink>,
}

impl LockWatch {
    /// A watch reporting `instance`'s lock waits to `sink`.
    pub fn new(instance: u64, sink: Arc<dyn DiagnosticSink>) -> Self {
        Self { instance, sink }
    }

    /// Lock `mutex`, which this instance calls `name`. A panic while it was held is an
    /// inconsistency the core already reported and panicked on (DI-D-46): the state is left as
    /// that panic found it, as [`crate::engine::lock`] does.
    pub fn lock<'a, T>(&'a self, mutex: &'a Mutex<T>, name: &'static str) -> WatchedGuard<'a, T> {
        let guard = match mutex.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                let Some(id) = CorrelationId::for_pair(&*self.sink, DiagnosticKind::LockWait)
                else {
                    return WatchedGuard::unwatched(
                        mutex.lock().unwrap_or_else(PoisonError::into_inner),
                    );
                };
                tagged(tag::LOCK_WATCH_LOCK_BEGAN, || {
                    self.sink.report(&Diagnostic::LockWaitBegan {
                        instance: self.instance,
                        lock: name,
                        id,
                    });
                });
                let taken = mutex.lock().unwrap_or_else(PoisonError::into_inner);
                // Written with the lock held, as the engineer ruled (DI-D-80): the sink's own
                // synchronization is a leaf beneath it, which `DiagnosticSink` requires.
                tagged(tag::LOCK_WATCH_LOCK_ENDED, || {
                    self.sink.report(&Diagnostic::LockWaitEnded {
                        instance: self.instance,
                        lock: name,
                        id,
                    });
                });
                taken
            }
        };
        WatchedGuard { guard }
    }
}

/// A locked mutex, taken through a [`LockWatch`] or without one.
pub struct WatchedGuard<'a, T> {
    guard: MutexGuard<'a, T>,
}

impl<'a, T> WatchedGuard<'a, T> {
    /// A guard whose acquisition is not reported: a lock taken with no watch.
    pub fn unwatched(guard: MutexGuard<'a, T>) -> Self {
        Self { guard }
    }
}

impl<T> Deref for WatchedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for WatchedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests;
