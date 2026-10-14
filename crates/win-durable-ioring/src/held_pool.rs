// Copyright (c) 2026 Mike Grier
//! A private thread pool's only thread, held busy, for the tests that must show a callback runs on
//! the pool a callback environment names (DI-9.8.1).
//!
//! Which pool ran a callback is not visible from the callback: either pool runs it. What is
//! visible is that a pool whose every thread is held runs nothing, so a delivery bound to such a
//! pool must wait, and one bound to the default pool would not.

use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::pool::ThreadpoolPool;
use windows_threadpool_sys::work::ThreadpoolWork;

/// Generous: the wait is for a pool callback to start.
const BOUND: Duration = Duration::from_secs(5);

/// A private pool limited to one thread, so that holding that thread holds the pool.
pub(crate) fn one_thread_pool() -> ThreadpoolPool {
    let pool = ThreadpoolPool::new().expect("create a pool");
    pool.set_max_threads(1)
        .expect("limit the pool to one thread");
    pool
}

type Gate = Arc<(Mutex<bool>, Condvar)>;

/// A work item holding the only thread of the pool a callback environment names, until released.
///
/// Dropping it releases it first, so a test that fails while it is held does not hang on a
/// delivery waiting for that thread.
pub(crate) struct Held {
    gate: Gate,
    _work: ThreadpoolWork,
}

impl Held {
    /// Hold the thread of `env`'s pool, returning once the thread is held.
    pub(crate) fn start(env: &mut CallbackEnviron<'_>) -> Self {
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (started_sender, started) = mpsc::channel();
        let held = Arc::clone(&gate);
        let work = ThreadpoolWork::new(
            move || {
                let _ = started_sender.send(());
                let (open, opened) = &*held;
                let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
                while !*open {
                    open = opened.wait(open).unwrap_or_else(PoisonError::into_inner);
                }
            },
            Some(env),
        )
        .expect("create the work item");
        work.submit();
        started
            .recv_timeout(BOUND)
            .expect("the pool's thread took the work item");
        Self { gate, _work: work }
    }

    /// Let the held thread go.
    pub(crate) fn release(&self) {
        let (open, opened) = &*self.gate;
        *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        opened.notify_all();
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.release();
    }
}
