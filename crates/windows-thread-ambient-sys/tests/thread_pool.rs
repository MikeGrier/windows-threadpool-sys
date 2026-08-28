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
