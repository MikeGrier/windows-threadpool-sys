// Copyright (c) 2026 Mike Grier
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
// The only user is gated off under `fail-fast` (dropping a submitted work item
// is required to panic there), so the import is unused in that build alone.
#[cfg_attr(feature = "fail-fast", allow(unused_imports))]
use std::time::Duration;

use crate::callback_env::CallbackEnviron;
use crate::work::ThreadpoolWork;

// --- creation ---

#[test]
fn new_with_no_env_succeeds() {
    let work = ThreadpoolWork::new(|| {}, None);
    assert!(work.is_ok());
}

#[test]
fn new_with_default_env_succeeds() {
    let mut env = CallbackEnviron::new();
    let work = ThreadpoolWork::new(|| {}, Some(&mut env));
    assert!(work.is_ok());
}

#[test]
fn new_env_with_runs_long_succeeds() {
    let mut env = CallbackEnviron::new();
    env.set_runs_long();
    let work = ThreadpoolWork::new(|| {}, Some(&mut env));
    assert!(work.is_ok());
}

// --- basic execution ---

#[test]
fn submit_once_callback_runs() {
    let ran = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&ran);
    let work = ThreadpoolWork::new(move || r.store(true, Ordering::SeqCst), None).unwrap();
    work.submit();
    work.wait();
    assert!(ran.load(Ordering::SeqCst));
}

#[test]
fn submit_increments_counter_once() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .unwrap();
    work.submit();
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn submit_five_times_counter_is_five() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .unwrap();
    for _ in 0..5 {
        work.submit();
    }
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 5);
}

#[test]
fn submit_ten_times_counter_is_ten() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .unwrap();
    for _ in 0..10 {
        work.submit();
    }
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 10);
}

// --- callback ownership model ---

/// Drop must wait for in-flight callbacks before freeing the captured context.
// Dropping a submitted work item is this test's subject, so it must keep doing
// exactly that. Under `fail-fast` that drop is *required* to panic, so the
// armed behaviour is guarded by the child-process test instead.
#[cfg(not(feature = "fail-fast"))]
#[test]
fn drop_waits_for_in_flight_callback() {
    let done = Arc::new(AtomicBool::new(false));
    let d = Arc::clone(&done);
    {
        let work = ThreadpoolWork::new(
            move || {
                std::thread::sleep(Duration::from_millis(5));
                d.store(true, Ordering::SeqCst);
            },
            None,
        )
        .unwrap();
        work.submit();
        // Drop here — must block until the callback sets the flag.
    }
    // If Drop didn't wait, the flag might still be false here (use-after-free territory).
    assert!(done.load(Ordering::SeqCst));
}

/// The captured closure may itself own heap-allocated state (proving the Arc count stays ≥ 1
/// throughout every invocation, i.e. no use-after-free on the context).
#[test]
fn callback_can_own_arc_data() {
    let data = Arc::new(vec![1u64, 2, 3, 4, 5]);
    let d = Arc::clone(&data);
    let sum = Arc::new(AtomicUsize::new(0));
    let s = Arc::clone(&sum);
    let work = ThreadpoolWork::new(
        move || {
            s.fetch_add(
                d.iter().map(|x| *x as usize).sum::<usize>(),
                Ordering::SeqCst,
            );
        },
        None,
    )
    .unwrap();
    work.submit();
    work.wait();
    assert_eq!(sum.load(Ordering::SeqCst), 15);
}

/// Context Arc strong-count during callback execution must be ≥ 2 (caller + callback).
#[test]
fn context_ref_count_valid_during_callback() {
    let data = Arc::new(AtomicUsize::new(0));
    let d = Arc::clone(&data);
    let inner_count = Arc::new(AtomicUsize::new(0));
    let ic = Arc::clone(&inner_count);
    let work = ThreadpoolWork::new(
        move || {
            // Strong count must be ≥ 2: the outer `data` plus this clone inside the closure.
            ic.store(Arc::strong_count(&d), Ordering::SeqCst);
        },
        None,
    )
    .unwrap();
    work.submit();
    work.wait();
    // At least 2 strong refs existed during the callback (outer + closure's captured clone).
    assert!(inner_count.load(Ordering::SeqCst) >= 2);
}

// --- cancel pending ---

/// cancel_pending should not panic; exact cancellation count is racy with the pool.
#[test]
fn cancel_pending_does_not_panic() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .unwrap();
    for _ in 0..10 {
        work.submit();
    }
    work.cancel_pending();
    // Count is in [0, 10] — we don't assert a specific value since cancellation is racy.
    assert!(count.load(Ordering::SeqCst) <= 10);
    // A dispatch never settles a work item's obligation -- it can be submitted
    // again -- so the drain is owed however many callbacks have run.
    work.stop_and_drain();
}

/// After cancel_pending, resubmit and wait — must run exactly once.
#[test]
fn resubmit_after_cancel_runs_once() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .unwrap();
    for _ in 0..5 {
        work.submit();
    }
    work.cancel_pending();
    // Reset counter to zero, then verify a fresh submission runs exactly once.
    count.store(0, Ordering::SeqCst);
    work.submit();
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

// --- wait / resubmit cycle ---

#[test]
fn wait_then_resubmit_counts_independently() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let work = ThreadpoolWork::new(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .unwrap();
    work.submit();
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    work.submit();
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn no_submit_drop_is_safe() {
    // Creating a work object and dropping it without submitting must not panic or block.
    let _work = ThreadpoolWork::new(|| {}, None).unwrap();
}

// --- callback environment integration ---

#[test]
fn work_with_env_callback_runs() {
    let ran = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&ran);
    let mut env = CallbackEnviron::new();
    let work =
        ThreadpoolWork::new(move || r.store(true, Ordering::SeqCst), Some(&mut env)).unwrap();
    work.submit();
    work.wait();
    assert!(ran.load(Ordering::SeqCst));
}

#[test]
fn work_with_runs_long_env_callback_runs() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let mut env = CallbackEnviron::new();
    env.set_runs_long();
    let work = ThreadpoolWork::new(
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
        Some(&mut env),
    )
    .unwrap();
    work.submit();
    work.wait();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// A cancel settles the drain obligation, as a wait does.
///
/// `cancel_pending` returns only once nothing is queued and nothing is
/// executing, which is the state the obligation tracks -- so leaving it owed
/// told a caller who had drained correctly that it had not, and under
/// `fail-fast` turned that false report into a panic from `Drop`.
///
/// Both directions, because a test that only checks the cancel would pass on an
/// implementation that never arms the obligation at all: the submit must arm
/// it, and the cancel must clear it.
#[test]
fn a_cancel_settles_the_drain_obligation_the_way_a_wait_does() {
    let work = ThreadpoolWork::new(|| {}, None).expect("create work");
    assert!(
        !work.obligation_owed(),
        "a work object that was never submitted owes no drain"
    );
    work.submit();
    assert!(
        work.obligation_owed(),
        "a submit is what makes the drain owed -- without this the next \
         assertion would hold for the wrong reason"
    );
    work.cancel_pending();
    assert!(
        !work.obligation_owed(),
        "cancel_pending returns with nothing queued and nothing executing, so \
         the obligation it leaves behind is a false one"
    );
}

/// The same object, submitted again after a cancel, owes the drain again.
///
/// The settle above must not be a one-way latch: `cancel_pending` leaves the
/// object reusable, and a later submit is a fresh obligation.
#[test]
fn a_submit_after_a_cancel_owes_the_drain_again() {
    let work = ThreadpoolWork::new(|| {}, None).expect("create work");
    work.submit();
    work.cancel_pending();
    work.submit();
    assert!(
        work.obligation_owed(),
        "the settle must clear the obligation, not disable the tracking"
    );
    work.stop_and_drain();
}
