// Copyright (c) 2026 Mike Grier
//! Unit tests for owned private thread pools.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::callback_env::CallbackEnviron;
use crate::pool::ThreadpoolPool;
use crate::work::ThreadpoolWork;

// --- creation ---

#[test]
fn new_pool_succeeds() {
    assert!(ThreadpoolPool::new().is_ok());
}

#[test]
fn several_pools_coexist() {
    let pools: Vec<ThreadpoolPool> = (0..8)
        .map(|_| ThreadpoolPool::new().expect("create pool"))
        .collect();
    assert_eq!(pools.len(), 8);
}

#[test]
fn drop_without_members_is_clean() {
    let pool = ThreadpoolPool::new().expect("create pool");
    drop(pool);
}

// --- thread limits ---

#[test]
fn set_max_threads_accepts_one() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_max_threads(1).expect("a non-zero maximum");
}

#[test]
fn set_max_threads_accepts_a_large_value() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_max_threads(512).expect("a non-zero maximum");
}

/// A maximum of zero leaves a pool that runs nothing at all: work submitted to
/// it is queued and never executed, and `SetThreadpoolThreadMaximum` returns
/// void, so nothing else could report it.
#[test]
fn set_max_threads_rejects_zero() {
    let pool = ThreadpoolPool::new().expect("create pool");
    let error = pool
        .set_max_threads(0)
        .expect_err("a maximum of zero must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the rejection is ours, not the platform's: {error}"
    );
    // The pool is unaffected and still usable.
    pool.set_max_threads(2).expect("a non-zero maximum");
}

/// A maximum below an established minimum is refused. Win32 would resolve the
/// conflict by last-call-wins and silently annul one of the two limits, so the
/// wrapper rejects the pair instead.
#[test]
fn a_maximum_below_the_minimum_is_rejected() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_min_threads(4).expect("set minimum");

    let error = pool
        .set_max_threads(2)
        .expect_err("a maximum below the minimum must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the rejection is ours, not the platform's: {error}"
    );

    // A maximum that can coexist with the minimum is still accepted.
    pool.set_max_threads(4)
        .expect("a maximum meeting the minimum");
}

/// A minimum above an established maximum is refused. Left to Win32 this is the
/// dangerous direction: the minimum is honoured and the maximum stops applying,
/// with no error and no way to observe it.
#[test]
fn a_minimum_above_the_maximum_is_rejected() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_max_threads(2).expect("set maximum");

    let error = pool
        .set_min_threads(4)
        .expect_err("a minimum above the maximum must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the rejection is ours, not the platform's: {error}"
    );

    // A minimum within the maximum is still accepted.
    pool.set_min_threads(2)
        .expect("a minimum within the maximum");
}

/// A limit we were never told cannot constrain its counterpart: Win32 offers no
/// way to read a pool's current limits back, so a fresh pool accepts either
/// setter at any value.
#[test]
fn an_unset_counterpart_does_not_constrain() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_min_threads(64)
        .expect("minimum with no maximum set");

    let other = ThreadpoolPool::new().expect("create pool");
    other
        .set_max_threads(1)
        .expect("maximum with no minimum set");
}

/// The maximum bounds concurrency when it is the effective limit. This is the
/// non-conflicting case; see `set_max_threads` for why the maximum is a
/// steady-state target rather than an instantaneous ceiling.
#[test]
fn the_maximum_bounds_concurrency() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_max_threads(2).expect("set maximum");

    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);

    let concurrent = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let inside = Arc::clone(&concurrent);
    let highest = Arc::clone(&peak);

    let work = ThreadpoolWork::new(
        move || {
            let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
            highest.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
            inside.fetch_sub(1, Ordering::SeqCst);
        },
        Some(&mut env),
    )
    .expect("create work");

    for _ in 0..8 {
        work.submit();
    }
    work.wait();

    assert!(
        peak.load(Ordering::SeqCst) <= 2,
        "the maximum was not honoured: peak was {}",
        peak.load(Ordering::SeqCst)
    );
}

#[test]
fn set_min_threads_zero_succeeds() {
    let pool = ThreadpoolPool::new().expect("create pool");
    assert!(pool.set_min_threads(0).is_ok());
}

#[test]
fn set_min_threads_one_succeeds() {
    let pool = ThreadpoolPool::new().expect("create pool");
    assert!(pool.set_min_threads(1).is_ok());
}

#[test]
fn min_and_max_together_are_accepted() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_max_threads(4).expect("a non-zero maximum");
    assert!(pool.set_min_threads(2).is_ok());
}

/// A rejected conflicting limit leaves the pool untouched and still usable: the
/// wrapper refuses the pair before calling into Win32, so nothing is half-applied.
#[test]
fn a_rejected_conflict_leaves_the_pool_usable() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_min_threads(4).expect("set minimum");
    pool.set_max_threads(1)
        .expect_err("a maximum below the minimum must be rejected");
    // Still usable afterwards, which is the property being checked here.
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);
    let ran = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ran);
    let work = ThreadpoolWork::new(
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
        },
        Some(&mut env),
    )
    .expect("create work");
    work.submit();
    work.wait();
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

// --- environment integration ---

#[test]
fn set_pool_stores_the_pools_value() {
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    assert_eq!(env.as_inner().Pool, 0, "the default pool is zero");
    env.set_pool(&pool);
    assert_ne!(env.as_inner().Pool, 0, "the private pool must be recorded");
}

#[test]
fn clear_pool_restores_the_default() {
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);
    env.clear_pool();
    assert_eq!(env.as_inner().Pool, 0);
}

#[test]
fn set_pool_does_not_disturb_other_fields() {
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    let version = env.as_inner().Version;
    let size = env.as_inner().Size;
    let priority = env.as_inner().CallbackPriority;
    env.set_pool(&pool);
    assert_eq!(env.as_inner().Version, version);
    assert_eq!(env.as_inner().Size, size);
    assert_eq!(env.as_inner().CallbackPriority, priority);
}

// --- callbacks actually run on the private pool ---

#[test]
fn work_runs_on_a_private_pool() {
    // Declared before the work object, so it is dropped last.
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_max_threads(2).expect("a non-zero maximum");

    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);

    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
        },
        Some(&mut env),
    )
    .expect("create work");

    for _ in 0..25 {
        work.submit();
    }
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 25);
}

/// A single-threaded pool still runs every submission, just not concurrently.
#[test]
fn a_single_thread_pool_runs_every_submission() {
    let pool = ThreadpoolPool::new().expect("create pool");
    pool.set_max_threads(1).expect("a non-zero maximum");
    pool.set_min_threads(1).expect("set minimum");

    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);

    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
        },
        Some(&mut env),
    )
    .expect("create work");

    for _ in 0..50 {
        work.submit();
    }
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 50);
}

/// Two pools serve their own objects independently.
#[test]
fn separate_pools_serve_their_own_objects() {
    let first_pool = ThreadpoolPool::new().expect("create first pool");
    let second_pool = ThreadpoolPool::new().expect("create second pool");

    let mut first_env = CallbackEnviron::new();
    first_env.set_pool(&first_pool);
    let mut second_env = CallbackEnviron::new();
    second_env.set_pool(&second_pool);

    let first_count = Arc::new(AtomicUsize::new(0));
    let first_counter = Arc::clone(&first_count);
    let first = ThreadpoolWork::new(
        move || {
            first_counter.fetch_add(1, Ordering::SeqCst);
        },
        Some(&mut first_env),
    )
    .expect("create first work");

    let second_count = Arc::new(AtomicUsize::new(0));
    let second_counter = Arc::clone(&second_count);
    let second = ThreadpoolWork::new(
        move || {
            second_counter.fetch_add(1, Ordering::SeqCst);
        },
        Some(&mut second_env),
    )
    .expect("create second work");

    for _ in 0..10 {
        first.submit();
    }
    for _ in 0..20 {
        second.submit();
    }
    first.wait();
    second.wait();

    assert_eq!(first_count.load(Ordering::SeqCst), 10);
    assert_eq!(second_count.load(Ordering::SeqCst), 20);
}

#[test]
fn a_pool_is_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<ThreadpoolPool>();
    assert_sync::<ThreadpoolPool>();
}
