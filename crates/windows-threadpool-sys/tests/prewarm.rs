// Copyright (c) 2026 Mike Grier
//! `prewarm_default_pool` makes the default pool create its first worker.
//!
//! **An integration test because it needs a process of its own.** The property
//! is a transition -- no worker to one worker -- and the pool is process-wide,
//! so any earlier test in the same binary warms it and makes the assertion
//! vacuous. Not hypothetical: written first as a unit test, it passed the whole
//! suite with the function sabotaged to warm nothing, because a sibling test had
//! already created the worker.
//!
//! **The default pool is identified by its thread maximum, not by handle
//! order.** A process holds more than one factory, this crate creates none of
//! the others, and the handle ordering between them is not a guarantee -- an
//! earlier version that took the lowest handle was flaky, reading a factory that
//! had nothing to do with the work being submitted.

#![cfg(windows)]
#![cfg(feature = "trace")]

use windows_threadpool_sys::pool::prewarm_default_pool;
use windows_threadpool_sys::trace::{WorkerFactorySnapshot, worker_factory_snapshot};

/// Smallest thread maximum that identifies the default process pool.
///
/// The default pool's maximum is large -- 768 where this was developed, derived
/// from the processor count -- while the other factory a process carries has a
/// maximum in the low single digits. Any threshold between the two separates
/// them, and this one is far from both so it does not depend on the exact value.
const DEFAULT_POOL_MIN_MAXIMUM: u32 = 64;

/// The default pool's `(total, waiting)` worker counts.
fn default_pool(snapshot: &[WorkerFactorySnapshot]) -> Option<(u32, u32)> {
    snapshot
        .iter()
        .find(|factory| factory.thread_maximum >= DEFAULT_POOL_MIN_MAXIMUM)
        .map(|factory| (factory.total_workers, factory.waiting_workers))
}

#[test]
fn prewarming_makes_the_default_pool_create_a_worker() {
    let before = worker_factory_snapshot();
    let (total_before, _) = default_pool(&before).unwrap_or_else(|| {
        panic!("no factory with a default-pool-sized maximum was found: {before:?}")
    });

    // The precondition. A fresh process has dispatched nothing, so the default
    // pool has never made a worker. If this fails, the test is running somewhere
    // it cannot measure what it claims to -- a real failure, not a reason to
    // weaken the assertion.
    assert_eq!(
        total_before, 0,
        "this test measures a cold-to-warm transition and needs a pool that has not yet made a \
         worker: {before:?}"
    );

    assert!(
        prewarm_default_pool(),
        "the warm-up callback did not run, so no worker is confirmed"
    );

    let after = worker_factory_snapshot();
    let (total_after, waiting_after) = default_pool(&after)
        .unwrap_or_else(|| panic!("the default pool must still be readable: {after:?}"));

    // Warming must *create* a worker, not merely find one. A sabotaged warm-up
    // that returns true without submitting leaves this at zero and fails here.
    assert!(
        total_after >= 1,
        "prewarming must leave the default pool holding a worker: {total_before} -> \
         {total_after}\n  before: {before:?}\n  after:  {after:?}"
    );

    // Self-consistency of the undocumented layout, so a misread surfaces here
    // rather than as a plausible-looking number somewhere else.
    assert!(
        waiting_after <= total_after,
        "more workers waiting than exist means the layout is misread: waiting={waiting_after} \
         total={total_after}"
    );
}
