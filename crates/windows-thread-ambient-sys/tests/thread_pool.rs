// Copyright (c) Mike Grier.

//! The composite, proved on a real Windows thread-pool worker.
//!
//! The unit tests apply state on ordinary spawned threads, which is enough to
//! check the mechanics. This file exists because the facts the crate was built
//! on were measured on a **thread-pool** worker specifically: a worker inherits
//! no impersonation token and runs with `SEM_FAILCRITICALERRORS` clear, and it
//! is *process-shared*, so contamination left behind is not confined to a thread
//! this test owns. That makes restoration a property worth proving where it
//! actually matters rather than only where it is convenient.
//!
//! These run on the process-default pool deliberately. A private pool would be
//! a gentler test of exactly the thing that must not go wrong.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_TOKEN, HANDLE};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::Diagnostics::Debug::GetThreadErrorMode;
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};
use windows_thread_ambient_sys::declared::MemoryPriority;
use windows_thread_ambient_sys::{AmbientState, CaptureSet, Declared, ThreadErrorMode};
use windows_threadpool_sys::work::ThreadpoolWork;

/// The live thread error mode, read straight from Win32.
fn live_mode() -> u32 {
    // SAFETY: the call takes no arguments and has no preconditions.
    unsafe { GetThreadErrorMode() }
}

/// Whether the calling thread currently carries an impersonation token.
fn has_token() -> bool {
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: `handle` is a valid writable destination; the pseudo-handle needs
    // no cleanup.
    let ok = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut handle) };
    if ok == 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(ERROR_NO_TOKEN as i32),
            "the only expected failure is that there is no token"
        );
        return false;
    }
    // SAFETY: the call above produced this handle and nothing else owns it.
    unsafe { CloseHandle(handle) };
    true
}

/// What one pool callback observed.
#[derive(Default)]
struct Observed {
    ran: AtomicBool,
    inherited_mode: AtomicU32,
    inherited_token: AtomicBool,
    applied_mode: AtomicU32,
    applied_token: AtomicBool,
    restored_mode: AtomicU32,
    restored_token: AtomicBool,
    restore_clean: AtomicBool,
}

/// Run `body` on a real pool worker and block until it has finished.
fn on_pool_worker<F>(body: F)
where
    F: Fn() + Send + Sync + 'static,
{
    let work = ThreadpoolWork::new(body, None).expect("create the pool work item");
    work.submit();
    work.wait();
}

#[test]
fn a_bare_pool_worker_inherits_none_of_the_submitters_state() {
    // The founding measurement, restated as a test because everything else here
    // depends on it: if a worker DID inherit the submitter's context, the crate
    // would have no reason to exist and the assertions below would pass for the
    // wrong reason.
    let guard = ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX
        .apply()
        .expect("install a distinctive mode on the submitting thread");
    assert_eq!(live_mode(), ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX.bits());

    let observed = Arc::new(Observed::default());
    let sink = Arc::clone(&observed);
    on_pool_worker(move || {
        sink.ran.store(true, Ordering::SeqCst);
        sink.inherited_mode.store(live_mode(), Ordering::SeqCst);
        sink.inherited_token.store(has_token(), Ordering::SeqCst);
    });
    guard.release().expect("restore the submitting thread");

    assert!(
        observed.ran.load(Ordering::SeqCst),
        "the callback never ran"
    );
    assert_eq!(
        observed.inherited_mode.load(Ordering::SeqCst),
        0,
        "a pool worker inherited the submitter's error mode"
    );
    assert!(
        !observed.inherited_token.load(Ordering::SeqCst),
        "a pool worker inherited an impersonation token"
    );
}

#[test]
fn a_captured_state_arrives_on_a_pool_worker_and_is_restored() {
    let guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .union(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX)
        .apply()
        .expect("install a distinctive mode to capture");
    let state = AmbientState::capture(CaptureSet::DEFAULT)
        .expect("capture")
        .with_declared(Declared::none().with_memory_priority(MemoryPriority::Low));
    guard.release().expect("restore the submitting thread");

    let observed = Arc::new(Observed::default());
    let sink = Arc::clone(&observed);
    on_pool_worker(move || {
        sink.ran.store(true, Ordering::SeqCst);
        sink.inherited_mode.store(live_mode(), Ordering::SeqCst);
        sink.inherited_token.store(has_token(), Ordering::SeqCst);

        let applied = state
            .with_applied(|| (live_mode(), has_token()))
            .expect("apply");
        sink.applied_mode.store(applied.value().0, Ordering::SeqCst);
        sink.applied_token
            .store(applied.value().1, Ordering::SeqCst);
        sink.restore_clean
            .store(applied.restore().is_clean(), Ordering::SeqCst);

        sink.restored_mode.store(live_mode(), Ordering::SeqCst);
        sink.restored_token.store(has_token(), Ordering::SeqCst);
    });

    assert!(
        observed.ran.load(Ordering::SeqCst),
        "the callback never ran"
    );

    // Arrived.
    let expected = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .union(ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX)
        .bits();
    assert_eq!(
        observed.applied_mode.load(Ordering::SeqCst),
        expected,
        "the captured error mode did not reach the worker"
    );
    assert!(
        observed.applied_token.load(Ordering::SeqCst),
        "the captured impersonation context did not reach the worker"
    );

    // Restored, which matters more here than anywhere else: this thread is
    // process-shared and goes back into the pool.
    assert!(
        observed.restore_clean.load(Ordering::SeqCst),
        "the worker reported a restore failure"
    );
    assert_eq!(
        observed.restored_mode.load(Ordering::SeqCst),
        observed.inherited_mode.load(Ordering::SeqCst),
        "the worker was returned to the pool with the wrong error mode"
    );
    assert_eq!(
        observed.restored_token.load(Ordering::SeqCst),
        observed.inherited_token.load(Ordering::SeqCst),
        "the worker was returned to the pool still carrying a token"
    );
}

#[test]
fn an_uncaptured_aspect_does_not_arrive_on_a_pool_worker() {
    // The negative the whole crate rests on. A suite that only ever watches
    // capture succeed cannot distinguish "the context was transported" from
    // "the worker happened to have it already" -- so here the submitter holds a
    // distinctive error mode that is deliberately NOT captured, and the worker
    // must not see it while still receiving the aspect that was captured.
    let guard = ThreadErrorMode::NO_GP_FAULT_ERROR_BOX
        .apply()
        .expect("install a mode that will not be captured");
    let state = AmbientState::capture(CaptureSet::IMPERSONATION).expect("capture");
    guard.release().expect("restore the submitting thread");

    let observed = Arc::new(Observed::default());
    let sink = Arc::clone(&observed);
    on_pool_worker(move || {
        sink.ran.store(true, Ordering::SeqCst);
        sink.inherited_mode.store(live_mode(), Ordering::SeqCst);
        let applied = state
            .with_applied(|| (live_mode(), has_token()))
            .expect("apply");
        sink.applied_mode.store(applied.value().0, Ordering::SeqCst);
        sink.applied_token
            .store(applied.value().1, Ordering::SeqCst);
    });

    assert!(
        observed.ran.load(Ordering::SeqCst),
        "the callback never ran"
    );
    assert_ne!(
        observed.applied_mode.load(Ordering::SeqCst),
        ThreadErrorMode::NO_GP_FAULT_ERROR_BOX.bits(),
        "an aspect that was never captured arrived on the worker anyway"
    );
    assert_eq!(
        observed.applied_mode.load(Ordering::SeqCst),
        observed.inherited_mode.load(Ordering::SeqCst),
        "an uncaptured aspect should leave the worker's own value alone"
    );
    assert!(
        observed.applied_token.load(Ordering::SeqCst),
        "the aspect that WAS captured failed to arrive, so this test proved nothing"
    );
}

#[test]
fn a_declared_aspect_is_installed_on_a_pool_worker_and_restored() {
    let state = AmbientState::capture(CaptureSet::NONE)
        .expect("capture")
        .with_declared(Declared::none().with_memory_priority(MemoryPriority::VeryLow));

    let observed = Arc::new(Observed::default());
    let sink = Arc::clone(&observed);
    on_pool_worker(move || {
        sink.ran.store(true, Ordering::SeqCst);
        let inherited = MemoryPriority::current().expect("readable");
        sink.inherited_mode
            .store(inherited.as_raw(), Ordering::SeqCst);

        let applied = state
            .with_applied(|| MemoryPriority::current().expect("readable"))
            .expect("apply");
        sink.applied_mode
            .store(applied.value().as_raw(), Ordering::SeqCst);
        sink.restore_clean
            .store(applied.restore().is_clean(), Ordering::SeqCst);

        let after = MemoryPriority::current().expect("readable");
        sink.restored_mode.store(after.as_raw(), Ordering::SeqCst);
    });

    assert!(
        observed.ran.load(Ordering::SeqCst),
        "the callback never ran"
    );
    assert_eq!(
        observed.applied_mode.load(Ordering::SeqCst),
        MemoryPriority::VeryLow.as_raw(),
        "the declared priority did not reach the worker"
    );
    assert!(observed.restore_clean.load(Ordering::SeqCst));
    assert_eq!(
        observed.restored_mode.load(Ordering::SeqCst),
        observed.inherited_mode.load(Ordering::SeqCst),
        "the worker was returned to the pool at the wrong memory priority"
    );
}

#[test]
fn one_state_serves_repeated_pool_callbacks() {
    // A captured state is reusable, which is what makes capture's cost bearable:
    // it is a snapshot, not a subscription.
    let guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install a distinctive mode");
    let state = Arc::new(AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture"));
    guard.release().expect("restore");

    let hits = Arc::new(AtomicU32::new(0));
    let misses = Arc::new(AtomicU32::new(0));
    let work_state = Arc::clone(&state);
    let work_hits = Arc::clone(&hits);
    let work_misses = Arc::clone(&misses);

    let work = ThreadpoolWork::new(
        move || {
            let applied = work_state.with_applied(live_mode).expect("apply");
            if *applied.value() == ThreadErrorMode::FAIL_CRITICAL_ERRORS.bits() {
                work_hits.fetch_add(1, Ordering::SeqCst);
            } else {
                work_misses.fetch_add(1, Ordering::SeqCst);
            }
        },
        None,
    )
    .expect("create the pool work item");

    for _ in 0..8 {
        work.submit();
    }
    work.wait();

    assert_eq!(
        misses.load(Ordering::SeqCst),
        0,
        "some callback saw the wrong mode"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 8, "not every callback ran");
}

// --- one capture, many workers (M23.5) -------------------------------------

/// Wait until `gauge` reaches `target`, or the deadline passes.
///
/// Used instead of a `Barrier` deliberately: a barrier inside the applied region
/// would hang the whole suite if one worker failed to apply and never arrived.
/// A bounded wait turns that into a failed assertion with a legible message.
fn wait_for(gauge: &AtomicU32, target: u32, within: std::time::Duration) -> u32 {
    let deadline = std::time::Instant::now() + within;
    loop {
        let seen = gauge.load(Ordering::SeqCst);
        if seen >= target || std::time::Instant::now() >= deadline {
            return seen;
        }
        std::thread::yield_now();
    }
}

#[test]
fn one_capture_serves_many_workers_simultaneously() {
    // The shape a traversal engine actually uses: capture once at submission,
    // share it, and run it on every worker at the same time. Each worker applies
    // and restores independently against one shared, immutable state.
    const WORKERS: u32 = 8;

    assert!(
        !has_token(),
        "precondition: the submitting thread has no token"
    );
    let guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install a distinctive mode to capture");
    let state = Arc::new(AmbientState::capture(CaptureSet::DEFAULT).expect("capture"));
    guard.release().expect("restore the submitting thread");

    // `arrived` is monotonic and is what the wait keys off. An earlier version
    // used a gauge that each worker decremented on the way out, which raced: the
    // last worker to arrive could decrement before the others observed the peak,
    // so they spun to the deadline and the assertion still passed on the single
    // sample the last worker recorded. Slow *and* weak.
    let arrived = Arc::new(AtomicU32::new(0));
    let inside = Arc::new(AtomicU32::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let saw_context = Arc::new(AtomicU32::new(0));
    let left_clean = Arc::new(AtomicU32::new(0));

    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let state = Arc::clone(&state);
            let arrived = Arc::clone(&arrived);
            let inside = Arc::clone(&inside);
            let peak = Arc::clone(&peak);
            let saw_context = Arc::clone(&saw_context);
            let left_clean = Arc::clone(&left_clean);
            std::thread::spawn(move || {
                let applied = state
                    .with_applied(|| {
                        arrived.fetch_add(1, Ordering::SeqCst);
                        inside.fetch_add(1, Ordering::SeqCst);
                        // Hold every worker inside the applied region at once,
                        // so the state is genuinely shared rather than merely
                        // reused in sequence. No worker leaves before the last
                        // one arrives, so a full count means real overlap.
                        let overlap =
                            wait_for(&arrived, WORKERS, std::time::Duration::from_secs(10));
                        peak.fetch_max(overlap, Ordering::SeqCst);
                        let observed = (live_mode(), has_token());
                        inside.fetch_sub(1, Ordering::SeqCst);
                        observed
                    })
                    .expect("apply on a worker");

                if applied.value().0 == ThreadErrorMode::FAIL_CRITICAL_ERRORS.bits()
                    && applied.value().1
                {
                    saw_context.fetch_add(1, Ordering::SeqCst);
                }
                if applied.restore().is_clean() {
                    left_clean.fetch_add(1, Ordering::SeqCst);
                }
                // Every worker must leave itself as it found it.
                assert_eq!(
                    live_mode(),
                    0,
                    "a worker was left with the wrong error mode"
                );
                assert!(!has_token(), "a worker was left carrying a token");
            })
        })
        .collect();

    for worker in workers {
        worker.join().expect("a worker panicked");
    }

    assert_eq!(
        peak.load(Ordering::SeqCst),
        WORKERS,
        "the workers never overlapped, so this proved reuse rather than sharing"
    );
    assert_eq!(
        inside.load(Ordering::SeqCst),
        0,
        "a worker left the applied region unbalanced"
    );
    assert_eq!(
        saw_context.load(Ordering::SeqCst),
        WORKERS,
        "not every worker saw the captured context"
    );
    assert_eq!(
        left_clean.load(Ordering::SeqCst),
        WORKERS,
        "not every worker restored cleanly"
    );
}

#[test]
fn one_shared_capture_serves_concurrent_pool_callbacks() {
    // The same property on the pool, at volume. Concurrency is observed rather
    // than required: the pool decides how many callbacks run at once, and
    // demanding a minimum here would make the test a measurement of the pool's
    // scheduling rather than of this crate.
    const SUBMISSIONS: u32 = 32;

    let guard = ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX
        .apply()
        .expect("install a distinctive mode to capture");
    let state = Arc::new(AmbientState::capture(CaptureSet::DEFAULT).expect("capture"));
    guard.release().expect("restore");

    let inside = Arc::new(AtomicU32::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let correct = Arc::new(AtomicU32::new(0));
    let clean = Arc::new(AtomicU32::new(0));
    let ran = Arc::new(AtomicU32::new(0));

    let work = {
        let state = Arc::clone(&state);
        let inside = Arc::clone(&inside);
        let peak = Arc::clone(&peak);
        let correct = Arc::clone(&correct);
        let clean = Arc::clone(&clean);
        let ran = Arc::clone(&ran);
        ThreadpoolWork::new(
            move || {
                ran.fetch_add(1, Ordering::SeqCst);
                let applied = state
                    .with_applied(|| {
                        let concurrent = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(concurrent, Ordering::SeqCst);
                        std::thread::yield_now();
                        let observed = (live_mode(), has_token());
                        inside.fetch_sub(1, Ordering::SeqCst);
                        observed
                    })
                    .expect("apply on a pool worker");

                if applied.value().0 == ThreadErrorMode::NO_OPEN_FILE_ERROR_BOX.bits()
                    && applied.value().1
                {
                    correct.fetch_add(1, Ordering::SeqCst);
                }
                if applied.restore().is_clean() {
                    clean.fetch_add(1, Ordering::SeqCst);
                }
            },
            None,
        )
        .expect("create the pool work item")
    };

    for _ in 0..SUBMISSIONS {
        work.submit();
    }
    work.wait();

    assert_eq!(
        ran.load(Ordering::SeqCst),
        SUBMISSIONS,
        "not every callback ran"
    );
    assert_eq!(
        correct.load(Ordering::SeqCst),
        SUBMISSIONS,
        "a callback did not see the captured context"
    );
    assert_eq!(
        clean.load(Ordering::SeqCst),
        SUBMISSIONS,
        "a callback did not restore cleanly"
    );
    assert_eq!(
        inside.load(Ordering::SeqCst),
        0,
        "a callback left the applied region unbalanced"
    );
    println!(
        "peak observed concurrency: {} of {SUBMISSIONS} submissions",
        peak.load(Ordering::SeqCst)
    );
}

#[test]
fn concurrent_workers_do_not_leak_state_into_each_other() {
    // Two states applied at once on different workers must not blend. Without
    // this, a per-thread implementation and a process-wide one would both pass:
    // each worker sees *a* captured mode, and only the cross-check shows it is
    // the right one.
    let first_guard = ThreadErrorMode::FAIL_CRITICAL_ERRORS
        .apply()
        .expect("install");
    let first = Arc::new(AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture"));
    first_guard.release().expect("restore");

    let second_guard = ThreadErrorMode::NO_GP_FAULT_ERROR_BOX
        .apply()
        .expect("install");
    let second = Arc::new(AmbientState::capture(CaptureSet::ERROR_MODE).expect("capture"));
    second_guard.release().expect("restore");

    let inside = Arc::new(AtomicU32::new(0));
    let run = |state: Arc<AmbientState>, inside: Arc<AtomicU32>| {
        std::thread::spawn(move || {
            state
                .with_applied(|| {
                    inside.fetch_add(1, Ordering::SeqCst);
                    let _ = wait_for(&inside, 2, std::time::Duration::from_secs(10));
                    live_mode()
                })
                .expect("apply")
                .into_value()
        })
    };

    let left = run(Arc::clone(&first), Arc::clone(&inside));
    let right = run(Arc::clone(&second), Arc::clone(&inside));
    let left = left.join().expect("no panic");
    let right = right.join().expect("no panic");

    assert_eq!(
        left,
        ThreadErrorMode::FAIL_CRITICAL_ERRORS.bits(),
        "the first worker saw the wrong state"
    );
    assert_eq!(
        right,
        ThreadErrorMode::NO_GP_FAULT_ERROR_BOX.bits(),
        "the second worker saw the wrong state"
    );
}
